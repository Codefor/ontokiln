//! SPARQL over a single immutable, materialized Pack v4.
//!
//! This is an in-memory dataset adapter, not another query evaluator. It uses
//! the existing Spareval engine and reads the same SPO/POS/OSP indices consumed
//! by the public Pack API. The Full query contract is not claimed until all
//! binding, derived-query and SHACL differential tests pass.
use anyhow::{Result, ensure};
use ontokiln_pack::v4::{DEFAULT_GRAPH, PackV4, View};
use oxrdf::Term;
use sparesults::{QueryResultsFormat, QueryResultsSerializer};
use spareval::{InternalQuad, QueryEvaluator, QueryResults, QueryableDataset};
use spargebra::{Query, SparqlParser};
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Eq, PartialEq, Hash)]
enum Encoded {
    Stored(u32),
    External(Term),
}

/// Ready-to-query immutable dataset. No graph copies or SQLite files are
/// constructed when a query is evaluated.
pub struct EngineV4 {
    pack: PackV4,
    named_asserted: Vec<u32>,
    named_inferred: Vec<u32>,
    named_closure: Vec<u32>,
}
impl EngineV4 {
    pub fn new(pack: PackV4) -> Self {
        let mut asserted = BTreeSet::new();
        let mut inferred = BTreeSet::new();
        for (row, quad) in pack.matching(None, None, None, None, View::Closure) {
            if quad[0] == DEFAULT_GRAPH {
                continue;
            }
            if pack.is_asserted(row) {
                asserted.insert(quad[0]);
            } else {
                inferred.insert(quad[0]);
            }
        }
        let named_closure: Vec<_> = asserted.union(&inferred).copied().collect();
        Self {
            pack,
            named_asserted: asserted.into_iter().collect(),
            named_inferred: inferred.into_iter().collect(),
            named_closure,
        }
    }

    pub fn load_pack(path: &Path) -> Result<Self> {
        Ok(Self::new(PackV4::load(path)?))
    }
    pub fn save_pack(&self, path: &Path) -> Result<()> {
        self.pack.save(path)
    }
    pub fn pack(&self) -> &PackV4 {
        &self.pack
    }

    pub fn execute(&self, query: &Query, view: View) -> Result<Vec<u8>> {
        // Query admission (local SELECT/ASK, no SERVICE/FROM) remains the
        // caller's responsibility, as for the existing Reasoner::write_sparql.
        ensure!(
            self.pack.metadata().fixpoint_reached || view == View::Asserted,
            "inferred/closure view requires completed materialization"
        );
        let names = match view {
            View::Asserted => &self.named_asserted,
            View::Inferred => &self.named_inferred,
            View::Closure => &self.named_closure,
        };
        let graph = Dataset {
            pack: &self.pack,
            view,
            names,
        };
        let results = QueryEvaluator::new().prepare(query).explain(graph).0?;
        let mut output = Vec::new();
        let serializer = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
        match results {
            QueryResults::Boolean(value) => {
                serializer.serialize_boolean_to_writer(&mut output, value)?;
            }
            QueryResults::Solutions(solutions) => {
                let mut writer = serializer
                    .serialize_solutions_to_writer(&mut output, solutions.variables().to_vec())?;
                for solution in solutions {
                    writer.serialize(&solution?)?;
                }
                writer.finish()?;
            }
            QueryResults::Graph(triples) => {
                for triple in triples {
                    writeln!(output, "{} .", triple?)?;
                }
            }
        }
        Ok(output)
    }

    pub fn select_or_ask(&self, text: &str, view: View) -> Result<Vec<u8>> {
        let query = SparqlParser::new().parse_query(text)?;
        ensure!(
            matches!(query, Query::Select { .. } | Query::Ask { .. }),
            "Pack v4 query requires SELECT or ASK"
        );
        self.execute(&query, view)
    }
}

struct Dataset<'a> {
    pack: &'a PackV4,
    view: View,
    names: &'a [u32],
}
impl<'a> QueryableDataset<'a> for Dataset<'a> {
    type InternalTerm = Encoded;
    type Error = io::Error;

    fn internalize_term(&self, term: Term) -> io::Result<Encoded> {
        Ok(self
            .pack
            .term_id(&term.to_string())
            .map_or(Encoded::External(term), Encoded::Stored))
    }
    fn externalize_term(&self, term: Encoded) -> io::Result<Term> {
        match term {
            Encoded::Stored(id) => self
                .pack
                .term(id)
                .ok_or_else(|| io::Error::other("invalid Pack v4 term ID"))?
                .parse::<Term>()
                .map_err(io::Error::other),
            Encoded::External(t) => Ok(t),
        }
    }
    fn internal_named_graphs(&self) -> impl Iterator<Item = io::Result<Encoded>> + use<'a> {
        self.names.iter().copied().map(|id| Ok(Encoded::Stored(id)))
    }
    fn contains_internal_graph_name(&self, graph: &Encoded) -> io::Result<bool> {
        Ok(matches!(graph, Encoded::Stored(id) if self.names.contains(id)))
    }
    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Encoded>,
        predicate: Option<&Encoded>,
        object: Option<&Encoded>,
        graph: Option<Option<&Encoded>>,
    ) -> impl Iterator<Item = io::Result<InternalQuad<Encoded>>> + use<'a> {
        type Rows<'b> = Box<dyn Iterator<Item = io::Result<InternalQuad<Encoded>>> + 'b>;
        let bound = |term: Option<&Encoded>| -> Option<Option<u32>> {
            match term {
                None => Some(None),
                Some(Encoded::Stored(id)) => Some(Some(*id)),
                Some(Encoded::External(_)) => None,
            }
        };
        let (Some(s), Some(p), Some(o)) = (bound(subject), bound(predicate), bound(object)) else {
            return Box::new(std::iter::empty()) as Rows<'a>;
        };
        let pack = self.pack;
        let view = self.view;
        match graph {
            Some(None) => Box::new(pack.union_triples(s, p, o, view).map(|[s, p, o]| {
                Ok(InternalQuad {
                    subject: Encoded::Stored(s),
                    predicate: Encoded::Stored(p),
                    object: Encoded::Stored(o),
                    graph_name: None,
                })
            })) as Rows<'a>,
            Some(Some(Encoded::Stored(graph_id))) => {
                if !self.names.contains(graph_id) {
                    return Box::new(std::iter::empty()) as Rows<'a>;
                }
                Box::new(pack.matching(s, p, o, Some(*graph_id), view).map(|(_, q)| {
                    Ok(InternalQuad {
                        subject: Encoded::Stored(q[1]),
                        predicate: Encoded::Stored(q[2]),
                        object: Encoded::Stored(q[3]),
                        graph_name: Some(Encoded::Stored(q[0])),
                    })
                })) as Rows<'a>
            }
            Some(Some(Encoded::External(_))) => Box::new(std::iter::empty()) as Rows<'a>,
            None => Box::new(
                pack.matching(s, p, o, None, view)
                    .filter(|(_, q)| q[0] != DEFAULT_GRAPH)
                    .map(|(_, q)| {
                        Ok(InternalQuad {
                            subject: Encoded::Stored(q[1]),
                            predicate: Encoded::Stored(q[2]),
                            object: Encoded::Stored(q[3]),
                            graph_name: Some(Encoded::Stored(q[0])),
                        })
                    }),
            ) as Rows<'a>,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontokiln_pack::v4::{Metadata, Statement};
    fn engine() -> EngineV4 {
        let terms = vec![
            "<urn:g>".into(),
            "<urn:o>".into(),
            "<urn:p>".into(),
            "<urn:s>".into(),
        ];
        let pack = PackV4::from_statements(
            terms,
            vec![
                Statement {
                    quad: [DEFAULT_GRAPH, 3, 2, 1],
                    asserted: true,
                    sources: vec![],
                },
                Statement {
                    quad: [0, 3, 2, 1],
                    asserted: true,
                    sources: vec![],
                },
            ],
            Metadata::new("no-rules-v1", true),
        )
        .unwrap();
        EngineV4::new(pack)
    }
    #[test]
    fn v4_select_uses_deduplicated_union_and_named_graphs() {
        let engine = engine();
        let query = "SELECT (COUNT(?s) AS ?n) WHERE { ?s <urn:p> <urn:o> }";
        let result = engine.select_or_ask(query, View::Closure).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(json["results"]["bindings"][0]["n"]["value"], "1");
        let named = engine
            .select_or_ask(
                "SELECT ?g WHERE { GRAPH ?g { <urn:s> <urn:p> <urn:o> } }",
                View::Closure,
            )
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&named).unwrap();
        assert_eq!(json["results"]["bindings"][0]["g"]["value"], "urn:g");
    }
    #[test]
    fn v4_ask_and_pack_reload_retain_results() {
        let engine = engine();
        let query = "ASK WHERE { <urn:s> <urn:p> <urn:o> }";
        let answer = engine.select_or_ask(query, View::Closure).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v4.pack");
        engine.save_pack(&path).unwrap();
        let restored = EngineV4::load_pack(&path).unwrap();
        assert_eq!(
            answer,
            restored.select_or_ask(query, View::Closure).unwrap()
        );
    }
}
