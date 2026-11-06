//! Read-only default RDF graph backed by the pack's existing SPO index.
use super::Reasoner;
use anyhow::{Result, ensure};
use ontokiln_pack::{Id, Pack};
use oxrdf::{Term, Variable};
use sparesults::{QueryResultsFormat, QueryResultsSerializer};
use spareval::{InternalQuad, QueryEvaluator, QueryResults, QueryableDataset};
use std::cell::Cell;
use std::io::{self, Write};

pub use spareval::CancellationToken;
pub use spargebra::{Query, SparqlParser};

const MAX_VISITS: usize = 100_000_000;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROWS: usize = 100_000;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Encoded {
    Stored(Id),
    // VALUES, expressions, query constants, etc. may introduce new terms.
    External(Term),
}

#[derive(Default)]
struct ScanStats {
    visits: Cell<usize>,
    full_scan_iterators: Cell<usize>,
}

impl ScanStats {
    fn budget_error(&self) -> io::Error {
        io::Error::other(format!(
            "SPARQL scan budget exceeded: attempted candidate {}, limit {MAX_VISITS}, full-scan iterators {}",
            self.visits.get(),
            self.full_scan_iterators.get(),
        ))
    }
}

struct Graph<'a> {
    pack: &'a Pack,
    stats: &'a ScanStats,
    cancel: &'a CancellationToken,
}

impl<'a> QueryableDataset<'a> for Graph<'a> {
    type InternalTerm = Encoded;
    type Error = io::Error;

    fn internalize_term(&self, term: Term) -> io::Result<Encoded> {
        Ok(match self.pack.lookup(&term.to_string()) {
            Some(id) => Encoded::Stored(id),
            None => Encoded::External(term),
        })
    }
    fn externalize_term(&self, term: Encoded) -> io::Result<Term> {
        match term {
            Encoded::Stored(id) => self.pack.term(id).parse().map_err(io::Error::other),
            Encoded::External(term) => Ok(term),
        }
    }
    fn internal_quads_for_pattern(
        &self,
        s: Option<&Encoded>,
        p: Option<&Encoded>,
        o: Option<&Encoded>,
        graph: Option<Option<&Encoded>>,
    ) -> impl Iterator<Item = io::Result<InternalQuad<Encoded>>> + use<'a> {
        let bound = [s, p, o];
        let ids = bound.map(|v| match v {
            Some(Encoded::Stored(id)) => Some(*id),
            _ => None,
        });
        let rows = if !matches!(graph, Some(None))
            || bound
                .iter()
                .any(|t| matches!(t, Some(Encoded::External(_))))
        {
            &[][..]
        } else {
            match ids {
                [Some(s), Some(p), _] => self.pack.edges(s, p),
                [Some(s), None, _] => self.pack.outgoing(s),
                _ => {
                    self.stats
                        .full_scan_iterators
                        .set(self.stats.full_scan_iterators.get() + 1);
                    self.pack.triples()
                }
            }
        };
        let mut iter = rows.iter();
        let stats = self.stats;
        let cancel = self.cancel;
        let mut done = false;
        std::iter::from_fn(move || {
            if done {
                return None;
            }
            for &[s, p, o] in iter.by_ref() {
                let n = stats.visits.get() + 1;
                stats.visits.set(n);
                if n > MAX_VISITS {
                    done = true;
                    return Some(Err(stats.budget_error()));
                }
                if n.is_multiple_of(1024) && cancel.is_cancelled() {
                    done = true;
                    return Some(Err(io::Error::other("SPARQL scan cancelled")));
                }
                if ids
                    .iter()
                    .zip([s, p, o])
                    .all(|(id, v)| id.is_none_or(|id| id == v))
                {
                    return Some(Ok(InternalQuad {
                        subject: Encoded::Stored(s),
                        predicate: Encoded::Stored(p),
                        object: Encoded::Stored(o),
                        graph_name: None,
                    }));
                }
            }
            None
        })
    }
}

struct Output(Vec<u8>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(io::Error::other("SPARQL output exceeds 4 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct SparqlOutput {
    pub media_type: &'static str,
    pub bytes: Vec<u8>,
}

impl Reasoner {
    pub fn sparql(
        &self,
        query: &Query,
        bindings: Vec<(Variable, Term)>,
        cancel: CancellationToken,
    ) -> Result<SparqlOutput> {
        let evaluator = QueryEvaluator::new().with_cancellation_token(cancel.clone());
        let mut prepared = evaluator.prepare(query);
        for (variable, term) in bindings {
            prepared = prepared.substitute_variable(variable, term);
        }
        let stats = ScanStats::default();
        let graph = Graph {
            pack: &self.pack,
            stats: &stats,
            cancel: &cancel,
        };
        let formatter = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
        let mut output = Output(Vec::new());
        let mut media_type = "application/sparql-results+json";
        match prepared.execute(graph)? {
            QueryResults::Boolean(value) => {
                formatter.serialize_boolean_to_writer(&mut output, value)?;
            }
            QueryResults::Solutions(solutions) => {
                let mut writer = formatter
                    .serialize_solutions_to_writer(&mut output, solutions.variables().to_vec())?;
                for (n, row) in solutions.enumerate() {
                    ensure!(n < MAX_ROWS, "SPARQL result exceeds row budget");
                    writer.serialize(&row?)?;
                }
                writer.finish()?;
            }
            QueryResults::Graph(triples) => {
                media_type = "application/n-triples";
                for (n, t) in triples.enumerate() {
                    ensure!(n < MAX_ROWS, "SPARQL graph exceeds row budget");
                    writeln!(output, "{} .", t?)?;
                }
            }
        }
        if stats.visits.get() > MAX_VISITS {
            return Err(stats.budget_error().into());
        }
        ensure!(!cancel.is_cancelled(), "SPARQL query cancelled");
        Ok(SparqlOutput {
            media_type,
            bytes: output.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_scan_identifies_the_request_wide_budget() {
        let pack = Pack::from_sorted(vec!["<urn:p>".into()], vec![[0, 0, 0]]).unwrap();
        let stats = ScanStats::default();
        stats.visits.set(MAX_VISITS - 1);
        let token = CancellationToken::new();
        let graph = Graph {
            pack: &pack,
            stats: &stats,
            cancel: &token,
        };
        assert!(
            graph
                .internal_quads_for_pattern(None, None, None, Some(None))
                .next()
                .unwrap()
                .is_ok()
        );
        let mut second = graph.internal_quads_for_pattern(None, None, None, Some(None));
        let message = second.next().unwrap().err().unwrap().to_string();
        assert!(
            message
                .contains("attempted candidate 100000001, limit 100000000, full-scan iterators 2")
        );
        assert!(second.next().is_none());
    }
}
