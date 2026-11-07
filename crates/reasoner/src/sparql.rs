//! Indexed explicit and RDFS views over one immutable snapshot.
use super::{Reasoner, View};
use anyhow::{Result, ensure};
use ontokiln_pack::{Id, Triple};
use oxrdf::{NamedNode, Term, Variable};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sparesults::{QueryResultsFormat, QueryResultsSerializer};
pub use spareval::CancellationToken;
use spareval::{InternalQuad, QueryEvaluator, QueryResults, QueryableDataset};
pub use spargebra::{Query, SparqlParser};
use std::{
    cell::Cell,
    io::{self, Write},
};

pub const MAX_VISITS: usize = 200_000_000;
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ROWS: usize = 100_000;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub view: View,
    pub revision: Option<String>,
    pub profile: bool,
    pub counters: bool,
    pub max_rows: usize,
    pub max_bytes: usize,
    pub max_visits: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            view: View::Asserted,
            revision: None,
            profile: false,
            counters: false,
            max_rows: MAX_ROWS,
            max_bytes: MAX_BYTES,
            max_visits: MAX_VISITS,
        }
    }
}
impl Options {
    pub fn validate(&self, jsonl: bool) -> Result<()> {
        let (rows, bytes) = if jsonl {
            (1_000_000, 64 * 1024 * 1024)
        } else {
            (MAX_ROWS, MAX_BYTES)
        };
        ensure!(
            self.max_visits <= MAX_VISITS && self.max_rows <= rows && self.max_bytes <= bytes,
            "limits exceed server policy"
        );
        Ok(())
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
enum Encoded {
    Stored(Id),
    External(Term),
}
thread_local! {static PROFILE_VISITS: Cell<u64>=const {Cell::new(0)};}
pub fn profile_visits() -> u64 {
    PROFILE_VISITS.get()
}

struct ScanStats {
    visits: Cell<usize>,
    full: Cell<usize>,
    indices: [Cell<usize>; 3],
    error: Cell<Option<&'static str>>,
}
impl ScanStats {
    fn new() -> Self {
        Self {
            visits: 0.into(),
            full: 0.into(),
            indices: Default::default(),
            error: None.into(),
        }
    }
    fn charge(&self, opts: &Options, cancel: &CancellationToken) -> Result<(), &'static str> {
        if let Some(e) = self.error.get() {
            return Err(e);
        }
        let n = self.visits.get() + 1;
        self.visits.set(n);
        if opts.profile {
            PROFILE_VISITS.set(n as u64);
        }
        let err = if n > opts.max_visits {
            Some("SPARQL scan budget exceeded")
        } else if cancel.is_cancelled() {
            Some("SPARQL query cancelled")
        } else {
            None
        };
        self.error.set(err);
        err.map_or(Ok(()), Err)
    }
    fn failure(&self, e: &str, opts: &Options) -> io::Error {
        if self.visits.get() > opts.max_visits {
            io::Error::other(format!(
                "SPARQL scan budget exceeded: attempted candidate {}, limit {}, full-scan iterators {}",
                self.visits.get(),
                opts.max_visits,
                self.full.get()
            ))
        } else {
            io::Error::other(e.to_owned())
        }
    }
}
struct Graph<'a> {
    engine: &'a Reasoner,
    stats: &'a ScanStats,
    cancel: &'a CancellationToken,
    options: &'a Options,
    names: &'a [Encoded],
}
impl<'a> QueryableDataset<'a> for Graph<'a> {
    type InternalTerm = Encoded;
    type Error = io::Error;
    fn internalize_term(&self, term: Term) -> io::Result<Encoded> {
        Ok(self
            .engine
            .pack
            .lookup(&term.to_string())
            .map_or(Encoded::External(term), Encoded::Stored))
    }
    fn externalize_term(&self, term: Encoded) -> io::Result<Term> {
        match term {
            Encoded::Stored(id) => self.engine.pack.term(id).parse().map_err(io::Error::other),
            Encoded::External(t) => Ok(t),
        }
    }
    fn internal_named_graphs(&self) -> impl Iterator<Item = io::Result<Encoded>> + use<'a> {
        let opts = self.options;
        let docs = self.engine.documents();
        self.names
            .iter()
            .enumerate()
            .filter(move |(i, _)| {
                opts.view == View::Asserted
                    && opts
                        .revision
                        .as_ref()
                        .is_none_or(|r| docs[*i].revision.as_ref() == Some(r))
            })
            .map(|(_, name)| Ok(name.clone()))
    }
    fn contains_internal_graph_name(&self, name: &Encoded) -> io::Result<bool> {
        Ok(self
            .internal_named_graphs()
            .any(|n| n.is_ok_and(|n| &n == name)))
    }
    fn internal_quads_for_pattern(
        &self,
        s: Option<&Encoded>,
        p: Option<&Encoded>,
        o: Option<&Encoded>,
        graph: Option<Option<&Encoded>>,
    ) -> impl Iterator<Item = io::Result<InternalQuad<Encoded>>> + use<'a> {
        let engine = self.engine;
        let opts = self.options;
        let stats = self.stats;
        let cancel = self.cancel;
        let bound = [s, p, o];
        let ids = bound.map(|t| match t {
            Some(Encoded::Stored(id)) => Some(*id),
            _ => None,
        });
        type Quads<'b> = Box<dyn Iterator<Item = io::Result<InternalQuad<Encoded>>> + 'b>;
        let unknown = bound
            .iter()
            .any(|t| matches!(t, Some(Encoded::External(_))));
        if !matches!(graph, Some(None)) {
            let names = self.names;
            let requested = graph
                .flatten()
                .and_then(|n| names.iter().position(|g| g == n));
            if unknown || opts.view != View::Asserted || (graph.is_some() && requested.is_none()) {
                return Box::new(std::iter::empty()) as Quads<'a>;
            }
            let candidates = engine.pack.candidates(ids);
            if ids == [None; 3] {
                stats.full.set(stats.full.get() + 1);
            }
            let idx = &stats.indices[candidates.order as usize];
            let mut done = false;
            let mut iter = candidates.flat_map(move |(row, [s, p, o])| {
                if let Err(e) = stats.charge(opts, cancel) {
                    return Box::new(std::iter::once(Err(stats.failure(e, opts)))) as Quads<'a>;
                }
                if opts.profile || opts.counters {
                    idx.set(idx.get() + 1)
                }
                Box::new(
                    engine
                        .pack
                        .source_ids(row)
                        .filter(move |&id| {
                            requested.is_none_or(|i| i == id as usize)
                                && opts.revision.as_ref().is_none_or(|r| {
                                    engine.documents()[id as usize].revision.as_ref() == Some(r)
                                })
                        })
                        .map(move |id| {
                            Ok(InternalQuad {
                                subject: Encoded::Stored(s),
                                predicate: Encoded::Stored(p),
                                object: Encoded::Stored(o),
                                graph_name: Some(names[id as usize].clone()),
                            })
                        }),
                )
            });
            return Box::new(std::iter::from_fn(move || {
                if done {
                    return None;
                }
                let q = iter.next()?;
                done = q.is_err();
                Some(q)
            })) as Quads<'a>;
        }
        let mut rows: Box<dyn Iterator<Item = io::Result<Triple>> + 'a> =
            Box::new(std::iter::empty());
        if !unknown {
            if opts.view == View::Asserted {
                let candidates = engine.pack.candidates(ids);
                if ids == [None; 3] {
                    stats.full.set(stats.full.get() + 1);
                }
                let idx = &stats.indices[candidates.order as usize];
                rows = Box::new(candidates.filter_map(move |(row, fact)| {
                    if let Err(e) = stats.charge(opts, cancel) {
                        return Some(Err(stats.failure(e, opts)));
                    }
                    if opts.profile || opts.counters {
                        idx.set(idx.get() + 1);
                    }
                    engine
                        .visible(row, opts.revision.as_deref())
                        .then_some(Ok(fact))
                }));
            } else {
                let mut cursor = 0;
                let subjects = std::iter::from_fn(move || {
                    if let Some(s) = ids[0] {
                        if cursor != 0 {
                            return None;
                        }
                        cursor = 1;
                        return Some(s);
                    }
                    let remaining = engine.pack.triples().get(cursor..)?;
                    let s = remaining.first()?[0];
                    cursor += remaining.partition_point(|t| t[0] == s);
                    Some(s)
                });
                rows = Box::new(subjects.flat_map(move |s| {
                    match engine.derive(s, opts.revision.as_deref(), true, &mut || {
                        stats.charge(opts, cancel)?;
                        if opts.profile || opts.counters {
                            let spo = &stats.indices[0];
                            spo.set(spo.get() + 1);
                        }
                        Ok(())
                    }) {
                        Ok(steps) => Box::new(
                            steps
                                .into_iter()
                                .filter(move |step| {
                                    opts.view == View::Closure || step.previous.is_some()
                                })
                                .map(|step| step.fact)
                                .filter(move |fact| {
                                    ids.iter()
                                        .zip(fact)
                                        .all(|(id, v)| id.is_none_or(|i| i == *v))
                                })
                                .map(Ok),
                        )
                            as Box<dyn Iterator<Item = io::Result<Triple>>>,
                        Err(e) => {
                            stats.error.set(Some(e));
                            Box::new(std::iter::once(Err(stats.failure(e, opts))))
                        }
                    }
                }));
            }
        }
        let mut done = false;
        Box::new(std::iter::from_fn(move || {
            if done {
                return None;
            }
            let row = rows.next()?;
            if row.is_err() {
                done = true;
            }
            Some(row.map(|[s, p, o]| InternalQuad {
                subject: Encoded::Stored(s),
                predicate: Encoded::Stored(p),
                object: Encoded::Stored(o),
                graph_name: None,
            }))
        })) as Quads<'a>
    }
}

struct Limited<'a> {
    out: &'a mut dyn Write,
    written: usize,
    limit: usize,
}
impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.written {
            return Err(io::Error::other("SPARQL output byte budget exceeded"));
        }
        let n = self.out.write(bytes)?;
        self.written += n;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
pub struct SparqlOutput {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}
pub fn media(query: &Query, jsonl: bool) -> &'static str {
    if jsonl {
        "application/x-ndjson"
    } else if matches!(query, Query::Construct { .. } | Query::Describe { .. }) {
        "application/n-triples"
    } else {
        "application/sparql-results+json"
    }
}
pub fn metered<T>(meter: fn() -> (u64, u64), f: impl FnOnce() -> T) -> T {
    spareval::metrics::with_query_meter(meter, f)
}
impl Reasoner {
    pub fn sparql(
        &self,
        query: &Query,
        bindings: Vec<(Variable, Term)>,
        cancel: CancellationToken,
    ) -> Result<SparqlOutput> {
        let mut bytes = Vec::new();
        self.write_sparql(
            query,
            bindings,
            &cancel,
            &Options::default(),
            false,
            &mut bytes,
        )?;
        Ok(SparqlOutput {
            bytes,
            media_type: media(query, false),
        })
    }
    pub fn write_sparql(
        &self,
        query: &Query,
        bindings: Vec<(Variable, Term)>,
        cancel: &CancellationToken,
        opts: &Options,
        jsonl: bool,
        out: &mut dyn Write,
    ) -> Result<Option<Value>> {
        opts.validate(jsonl)?;
        self.check_revision(opts.revision.as_deref())
            .map_err(anyhow::Error::msg)?;
        ensure!(!cancel.is_cancelled(), "SPARQL query cancelled");
        let mut evaluator = QueryEvaluator::new().with_cancellation_token(cancel.clone());
        if opts.profile {
            PROFILE_VISITS.set(0);
            evaluator = evaluator.compute_statistics();
        }
        let mut prepared = evaluator.prepare(query);
        for (v, t) in bindings {
            prepared = prepared.substitute_variable(v, t);
        }
        let stats = ScanStats::new();
        let names: Vec<_> = self
            .documents()
            .iter()
            .map(|d| {
                let term = Term::NamedNode(NamedNode::new_unchecked(d.graph()));
                self.pack
                    .lookup(&term.to_string())
                    .map_or(Encoded::External(term), Encoded::Stored)
            })
            .collect();
        let graph = Graph {
            engine: self,
            stats: &stats,
            cancel,
            options: opts,
            names: &names,
        };
        let (results, plan) = prepared.explain(graph);
        let mut out = Limited {
            out,
            written: 0,
            limit: opts.max_bytes,
        };
        let mut n = 0;
        let mut row = || -> Result<()> {
            ensure!(n < opts.max_rows, "SPARQL output row budget exceeded");
            n += 1;
            ensure!(!cancel.is_cancelled(), "SPARQL query cancelled");
            Ok(())
        };
        let formatter = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
        match results? {
            QueryResults::Boolean(value) => {
                row()?;
                if jsonl {
                    writeln!(out, "{}", json!({"boolean":value}))?
                } else {
                    formatter.serialize_boolean_to_writer(&mut out, value)?;
                }
            }
            QueryResults::Solutions(solutions) => {
                if jsonl {
                    for solution in solutions {
                        row()?;
                        let solution = solution?;
                        let bindings: std::collections::BTreeMap<_, _> = solution
                            .iter()
                            .map(|(v, t)| (v.as_str(), t.to_string()))
                            .collect();
                        serde_json::to_writer(&mut out, &json!({"bindings":bindings}))?;
                        out.write_all(b"\n")?;
                        out.flush()?;
                    }
                } else {
                    let mut writer = formatter
                        .serialize_solutions_to_writer(&mut out, solutions.variables().to_vec())?;
                    for solution in solutions {
                        row()?;
                        writer.serialize(&solution?)?;
                    }
                    writer.finish()?;
                }
            }
            QueryResults::Graph(triples) => {
                for triple in triples {
                    row()?;
                    let t = triple?;
                    if jsonl {
                        writeln!(out, "{}", json!({"triple":t.to_string()}))?;
                        out.flush()?;
                    } else {
                        writeln!(out, "{t} .")?;
                    }
                }
            }
        }
        if let Some(e) = stats.error.get() {
            return Err(stats.failure(e, opts).into());
        }
        ensure!(!cancel.is_cancelled(), "SPARQL query cancelled");
        if !opts.profile && !opts.counters {
            return Ok(None);
        }
        let mut report = json!({"candidate_rows":stats.visits.get(),"index_rows":{"SPO":stats.indices[0].get(),"POS":stats.indices[1].get(),"OSP":stats.indices[2].get()},"output_rows":n,"output_bytes":out.written});
        if opts.profile {
            let mut raw = Vec::new();
            plan.write_in_json(&mut raw)?;
            report["execution"] = serde_json::from_slice(&raw)?;
        }
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_wide_inclusive_boundary() {
        let stats = ScanStats::new();
        stats.visits.set(MAX_VISITS - 1);
        let opts = Options::default();
        let cancel = CancellationToken::new();
        stats.charge(&opts, &cancel).unwrap();
        let e = stats.charge(&opts, &cancel).unwrap_err();
        assert!(
            stats
                .failure(e, &opts)
                .to_string()
                .contains("attempted candidate 200000001, limit 200000000")
        );
    }
}
