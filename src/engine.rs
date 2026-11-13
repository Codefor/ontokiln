//! Embeddable, immutable in-memory ontology engine.
//! Compilation returns a live Reasoner and a persistable Pack, not a service.
//! SHACL shares a lazily built validation graph across requests.
use crate::rdf_graph::{Graph, RDF, SH, iri};
use anyhow::{Context, Result, ensure};
use ontokiln_compiler::{
    v4::{Source, compile_sources},
    v4_materialize::{Materialization, Rule, materialize},
};
use ontokiln_pack::v4::{PackV4, View};
use ontokiln_reasoner::{
    parameters::{self, Parameter, ValuesTable},
    v4::EngineV4,
};
use oxigraph::io::{RdfFormat, RdfParser};
use oxrdf::{Term, Variable};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

pub struct OntologyEngine {
    reasoner: EngineV4,
    validation_graph: OnceLock<Graph>,
}
impl OntologyEngine {
    /// Build and fully materialize a new ontology. Failure never publishes Pack.
    pub fn build(
        sources: &[Source],
        profile: &str,
        rules: &[Rule],
    ) -> Result<(Self, Materialization)> {
        let asserted = compile_sources(sources)?;
        let (pack, stats) = materialize(asserted, profile, rules)?;
        Ok((Self::from_pack(pack)?, stats))
    }
    pub fn load_pack(path: &Path) -> Result<Self> {
        Self::from_pack(PackV4::load(path)?)
    }
    pub fn from_pack(pack: PackV4) -> Result<Self> {
        ensure!(
            pack.metadata().fixpoint_reached,
            "complete materialization required for OntologyEngine"
        );
        Ok(Self {
            reasoner: EngineV4::new(pack),
            validation_graph: OnceLock::new(),
        })
    }
    /// Saving does not evict or rebuild the live Reasoner.
    pub fn save_pack(&self, path: &Path) -> Result<()> {
        self.reasoner.save_pack(path)
    }
    pub fn pack(&self) -> &PackV4 {
        self.reasoner.pack()
    }
    /// Local SELECT/ASK. Output is the standard SPARQL Results JSON.
    pub fn query(&self, query: &str, view: View) -> Result<Vec<u8>> {
        crate::query::local_query(query)?;
        self.reasoner.select_or_ask(query, view)
    }
    /// Apply typed query parameters, VALUES tables and RDF initial bindings.
    /// Uses the same parser/algebra binding machinery as API v1 Full.
    /// Binding-only variables are never added to public SELECT columns.
    pub fn query_bound(
        &self,
        query: &str,
        view: View,
        parameters: &BTreeMap<String, Parameter>,
        rdf_bindings: &BTreeMap<String, String>,
        values: &[ValuesTable],
    ) -> Result<Vec<u8>> {
        self.query_with_overlay(query, view, parameters, rdf_bindings, values, &[])
    }

    fn query_with_overlay(
        &self,
        query: &str,
        view: View,
        parameters: &BTreeMap<String, Parameter>,
        rdf_bindings: &BTreeMap<String, String>,
        values: &[ValuesTable],
        overlay: &[[Term; 3]],
    ) -> Result<Vec<u8>> {
        crate::query::local_query(query)?;
        let mut parsed = parameters::bind_tables(
            spargebra::SparqlParser::new().parse_query(query)?,
            parameters,
            values,
        )?;
        // ASK does not expose output columns; its algebra consumes initial
        // bindings directly. SELECT needs hidden bindings projected internally.
        let public_columns =
            if !rdf_bindings.is_empty() && matches!(parsed, spargebra::Query::Select { .. }) {
                crate::query::expose_bindings(&mut parsed, rdf_bindings.keys().cloned())?
            } else {
                None
            };
        let bound_terms = rdf_bindings
            .iter()
            .map(|(name, value)| Ok((Variable::new(name)?, value.parse::<Term>()?)))
            .collect::<Result<Vec<_>>>()?;
        let bytes = self
            .reasoner
            .execute_bound_overlay(&parsed, view, &bound_terms, overlay)?;
        if let Some(columns) = public_columns {
            let mut reply: Value = serde_json::from_slice(&bytes)?;
            let keep: std::collections::BTreeSet<_> = columns.iter().cloned().collect();
            let rows = reply["results"]["bindings"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("SELECT result is missing bindings"))?;
            for row in rows {
                let row = row
                    .as_object_mut()
                    .ok_or_else(|| anyhow::anyhow!("invalid SELECT result row"))?;
                row.retain(|name, _| keep.contains(name));
            }
            reply["head"]["vars"] = json!(columns);
            return Ok(serde_json::to_vec(&reply)?);
        }
        Ok(bytes)
    }

    /// Execute an ordered chain of local CONSTRUCT or SELECT s/p/o assets.
    /// New facts are only visible to this request's subsequent derived queries
    /// and final SELECT/ASK. The live Pack and its named graphs are immutable.
    pub fn query_with_derived(
        &self,
        query: &str,
        view: View,
        parameters: &BTreeMap<String, Parameter>,
        rdf_bindings: &BTreeMap<String, String>,
        values: &[ValuesTable],
        derived_queries: &[String],
        max_derived_facts: u64,
        max_intermediate_rows: u64,
    ) -> Result<Vec<u8>> {
        use oxrdf::NamedOrBlankNode;
        use std::collections::HashSet;
        ensure!(
            derived_queries.len() <= 32,
            "INVALID_REQUEST|at most 32 derived queries are allowed"
        );
        let mut overlay = Vec::<[Term; 3]>::new();
        let mut unique = HashSet::<[Term; 3]>::new();
        let mut visited = 0_u64;
        let bindings = rdf_bindings
            .iter()
            .map(|(name, value)| Ok((Variable::new(name)?, value.parse::<Term>()?)))
            .collect::<Result<Vec<_>>>()?;
        for text in derived_queries {
            crate::query::local_derived_query(text)?;
            let mut prepared = parameters::bind_tables(
                spargebra::SparqlParser::new().parse_query(text)?,
                parameters,
                values,
            )?;
            let is_select = matches!(prepared, spargebra::Query::Select { .. });
            if is_select && !rdf_bindings.is_empty() {
                crate::query::expose_bindings(&mut prepared, rdf_bindings.keys().cloned())?;
            }
            let output = self
                .reasoner
                .execute_bound_overlay(&prepared, view, &bindings, &overlay)?;
            let mut add = |fact: [Term; 3]| -> Result<()> {
                visited = visited.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("BUDGET_EXHAUSTED|derived intermediate counter overflow")
                })?;
                ensure!(
                    visited <= max_intermediate_rows,
                    "BUDGET_EXHAUSTED|derived query intermediate row limit"
                );
                if unique.insert(fact.clone()) {
                    ensure!(
                        unique.len() as u64 <= max_derived_facts,
                        "BUDGET_EXHAUSTED|derived query fact limit"
                    );
                    overlay.push(fact);
                }
                Ok(())
            };
            if is_select {
                let value: Value = serde_json::from_slice(&output)?;
                for var in ["s", "p", "o"] {
                    ensure!(
                        value["head"]["vars"]
                            .as_array()
                            .is_some_and(|vars| vars.contains(&json!(var))),
                        "INVALID_QUERY|derived SELECT requires s, p and o columns"
                    );
                }
                for row in value["results"]["bindings"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("INVALID_QUERY|missing derived result rows"))?
                {
                    let terms = crate::query::rdf_triple(row)?;
                    add([
                        terms[0].parse::<Term>()?,
                        terms[1].parse::<Term>()?,
                        terms[2].parse::<Term>()?,
                    ])?;
                }
            } else {
                // Fresh blank-node scope for each CONSTRUCT asset, including
                // repeated template blank nodes across different assets.
                let parser = RdfParser::from_format(RdfFormat::NTriples).rename_blank_nodes();
                for triple in parser.for_reader(output.as_slice()) {
                    let quad = triple?;
                    let subject = match quad.subject {
                        NamedOrBlankNode::NamedNode(n) => Term::NamedNode(n),
                        NamedOrBlankNode::BlankNode(b) => Term::BlankNode(b),
                    };
                    add([subject, Term::NamedNode(quad.predicate), quad.object])?;
                }
            }
        }
        self.query_with_overlay(query, view, parameters, rdf_bindings, values, &overlay)
    }

    /// Execute a local CONSTRUCT or SELECT s/p/o derived-query asset.
    /// Results are request-scoped; this never changes the immutable Pack.
    pub fn derived_query(&self, query: &str, view: View) -> Result<Vec<u8>> {
        crate::query::local_derived_query(query)?;
        let parsed = spargebra::SparqlParser::new().parse_query(query)?;
        self.reasoner.execute(&parsed, view)
    }

    fn data_graph(&self) -> &Graph {
        self.validation_graph.get_or_init(|| {
            let pack = self.pack();
            Graph {
                triples: pack
                    .union_triples(None, None, None, View::Closure)
                    .map(|[s, p, o]| {
                        [
                            pack.term(s).expect("validated Pack subject ID").to_owned(),
                            pack.term(p)
                                .expect("validated Pack predicate ID")
                                .to_owned(),
                            pack.term(o).expect("validated Pack object ID").to_owned(),
                        ]
                    })
                    .collect(),
            }
        })
    }
    /// Parse and validate the supplied frozen external Shapes source files.
    /// It accepts the same four local RDF source formats as Full. Each source
    /// has its own blank-node scope, and no file bytes are trusted without
    /// checking the caller's SHA-256 precondition.
    pub fn validate_shape_sources(
        &self,
        sources: &[Source],
        selected_shapes: Option<&BTreeSet<String>>,
        focus_node: Option<&str>,
        max_depth: usize,
    ) -> Result<Value> {
        let mut shapes = Graph::default();
        for source in sources {
            let meta = fs::symlink_metadata(&source.path)?;
            ensure!(
                meta.file_type().is_file(),
                "SHACL source must be a regular non-symlink file"
            );
            let bytes = fs::read(&source.path)?;
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == source.expected_sha256,
                "SHACL source digest mismatch: {}",
                source.document_id
            );
            let format = match source.format.as_str() {
                "turtle" => RdfFormat::Turtle,
                "ntriples" => RdfFormat::NTriples,
                "trig" => RdfFormat::TriG,
                "nquads" => RdfFormat::NQuads,
                _ => anyhow::bail!("unsupported SHACL RDF source format"),
            };
            let mut parser = RdfParser::from_format(format).rename_blank_nodes();
            if let Some(base) = source.base_iri.as_deref() {
                parser = parser.with_base_iri(base)?;
            }
            for quad in parser.for_reader(bytes.as_slice()) {
                let quad = quad.with_context(|| {
                    format!("SHACL source parse failed: {}", source.document_id)
                })?;
                shapes.triples.insert([
                    quad.subject.to_string(),
                    quad.predicate.to_string(),
                    quad.object.to_string(),
                ]);
            }
        }
        self.validate(Some(&shapes), selected_shapes, focus_node, max_depth)
    }

    /// Validate external Shapes, or Shapes within the data graph when None.
    /// The data graph is the distinct union of every asserted/inferred quad,
    /// independent of the query view, matching Full's SHACL semantics.
    pub fn validate(
        &self,
        shapes: Option<&Graph>,
        selected_shapes: Option<&BTreeSet<String>>,
        focus_node: Option<&str>,
        max_depth: usize,
    ) -> Result<Value> {
        let data = self.data_graph();
        let shapes = shapes.unwrap_or(data);
        let selected = selected_shapes.cloned().unwrap_or_else(|| {
            let rdf_type = iri(&format!("{RDF}type"));
            let node = iri(&format!("{SH}NodeShape"));
            let property = iri(&format!("{SH}PropertyShape"));
            shapes
                .triples
                .iter()
                .filter(|t| t[1] == rdf_type && (t[2] == node || t[2] == property))
                .map(|t| t[0].clone())
                .collect()
        });
        let report = crate::shacl::validate_with_depth(
            data, shapes, &selected, focus_node, max_depth, true,
        )?;
        Ok(json!({
            "conforms":report["conforms"],
            "checked_shapes":selected.len(),
            "violations":report["violations"].as_array().map_or(0,Vec::len),
            "report":report
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    use tempfile::tempdir;

    fn source(dir: &Path, name: &str, data: &str) -> Source {
        let path = dir.join(name);
        fs::write(&path, data).unwrap();
        Source {
            path,
            document_id: name.into(),
            format: "turtle".into(),
            expected_sha256: format!("{:x}", Sha256::digest(data.as_bytes())),
            base_iri: None,
        }
    }
    #[test]
    fn build_save_reload_and_query_completed_rdfs() {
        let dir = tempdir().unwrap();
        let rdf = concat!(
            "<urn:Dog> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Animal> .\n",
            "<urn:Animal> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Life> .\n",
            "<urn:Fido> a <urn:Dog> .\n"
        );
        let (engine, stats) = OntologyEngine::build(
            &[source(dir.path(), "data.ttl", rdf)],
            "rdfs-supported-v1",
            &[],
        )
        .unwrap();
        assert!(stats.inferred_facts >= 2);
        let query = "ASK WHERE { <urn:Fido> a <urn:Life> }";
        let before = engine.query(query, View::Closure).unwrap();
        let parsed: Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(parsed["boolean"], true);
        let path = dir.path().join("reasoned.pack");
        engine.save_pack(&path).unwrap();
        assert_eq!(before, engine.query(query, View::Closure).unwrap());
        let restored = OntologyEngine::load_pack(&path).unwrap();
        assert_eq!(before, restored.query(query, View::Closure).unwrap());
    }
    #[test]
    fn construct_asset_executes_without_mutating_pack() {
        let dir = tempdir().unwrap();
        let input = source(dir.path(), "data.ttl", "<urn:s> <urn:p> <urn:o> .");
        let (engine, _) = OntologyEngine::build(&[input], "no-rules-v1", &[]).unwrap();
        let before = engine.pack().quad_count();
        let construct = "CONSTRUCT { <urn:derived> <urn:p> ?o } WHERE { <urn:s> <urn:p> ?o }";
        let result = engine.derived_query(construct, View::Closure).unwrap();
        let value = String::from_utf8(result).unwrap();
        assert!(value.contains("<urn:derived> <urn:p> <urn:o>"));
        assert_eq!(engine.pack().quad_count(), before);
        let ask = "ASK { <urn:derived> <urn:p> <urn:o> }";
        let response = engine.query(ask, View::Closure).unwrap();
        let response: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(response["boolean"], false);
    }

    #[test]
    fn derived_chain_is_request_local_and_immutable() {
        let dir = tempdir().unwrap();
        let (engine, _) = OntologyEngine::build(
            &[source(dir.path(), "input.ttl", "<urn:s> <urn:p> <urn:o> .")],
            "no-rules-v1",
            &[],
        )
        .unwrap();
        let before = engine.pack().quad_count();
        let derived = vec![
            "CONSTRUCT { <urn:d> <urn:p> ?o } WHERE { <urn:s> <urn:p> ?o }".into(),
            "CONSTRUCT { <urn:e> <urn:p> ?o } WHERE { <urn:d> <urn:p> ?o }".into(),
        ];
        let q = "ASK { <urn:e> <urn:p> <urn:o> }";
        let result = engine
            .query_with_derived(
                q,
                View::Closure,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[],
                &derived,
                10,
                10,
            )
            .unwrap();
        let answer: Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(answer["boolean"], true);
        let unchanged: Value =
            serde_json::from_slice(&engine.query(q, View::Closure).unwrap()).unwrap();
        assert_eq!(unchanged["boolean"], false);
        assert_eq!(engine.pack().quad_count(), before);
        assert!(
            engine
                .query_with_derived(
                    q,
                    View::Closure,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &[],
                    &derived,
                    1,
                    10
                )
                .is_err()
        );
    }

    #[test]
    fn select_spo_overlay_supports_duplicate_rows_without_duplicate_facts() {
        let dir = tempdir().unwrap();
        let (engine, _) = OntologyEngine::build(
            &[source(dir.path(), "input.ttl", "<urn:s> <urn:p> <urn:o> .")],
            "no-rules-v1",
            &[],
        )
        .unwrap();
        let queries=vec![
            "SELECT ?s ?p ?o WHERE { VALUES (?s ?p ?o) { (<urn:d> <urn:p> <urn:o>) (<urn:d> <urn:p> <urn:o>) } }".to_string()
        ];
        let q = "ASK { <urn:d> <urn:p> <urn:o> }";
        assert!(
            engine
                .query_with_derived(
                    q,
                    View::Closure,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                    &[],
                    &queries,
                    1,
                    1
                )
                .is_err()
        );
        let value = engine
            .query_with_derived(
                q,
                View::Closure,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[],
                &queries,
                1,
                2,
            )
            .unwrap();
        let reply: Value = serde_json::from_slice(&value).unwrap();
        assert_eq!(reply["boolean"], true);
    }

    #[test]
    fn full_shacl_core_runs_on_memory_pack() {
        let dir = tempdir().unwrap();
        let (engine, _) = OntologyEngine::build(
            &[source(
                dir.path(),
                "data.ttl",
                "<urn:person> <urn:age> \"42\" .",
            )],
            "no-rules-v1",
            &[],
        )
        .unwrap();
        let shapes = Graph {
            triples: [
                [
                    "<urn:Shape>".into(),
                    iri(&format!("{RDF}type")),
                    iri(&format!("{SH}NodeShape")),
                ],
                [
                    "<urn:Shape>".into(),
                    iri(&format!("{SH}targetNode")),
                    "<urn:person>".into(),
                ],
                [
                    "<urn:Shape>".into(),
                    iri(&format!("{SH}property")),
                    "_:p0".into(),
                ],
                [
                    "_:p0".into(),
                    iri(&format!("{SH}path")),
                    "<urn:name>".into(),
                ],
                [
                    "_:p0".into(),
                    iri(&format!("{SH}minCount")),
                    "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>".into(),
                ],
            ]
            .into_iter()
            .collect(),
        };
        let result = engine.validate(Some(&shapes), None, None, 64).unwrap();
        assert_eq!(result["checked_shapes"], 1);
        assert_eq!(result["conforms"], false);
        assert!(result["violations"].as_u64().unwrap() > 0);
        assert!(engine.validation_graph.get().is_some());
        assert_eq!(
            result,
            engine.validate(Some(&shapes), None, None, 64).unwrap()
        );
    }
    #[test]
    fn external_shapes_are_loaded_and_digest_checked() {
        let dir = tempdir().unwrap();
        let (engine, _) = OntologyEngine::build(
            &[source(
                dir.path(),
                "data.ttl",
                "<urn:person> <urn:age> \"42\" .",
            )],
            "no-rules-v1",
            &[],
        )
        .unwrap();
        let shapes = format!(
            "<urn:Shape> a <{SH}NodeShape> .\n\
             <urn:Shape> <{SH}targetNode> <urn:person> .\n\
             <urn:Shape> <{SH}property> _:p .\n\
             _:p <{SH}path> <urn:name> .\n\
             _:p <{SH}minCount> 1 .\n"
        );
        let spec = source(dir.path(), "shape.ttl", &shapes);
        let result = engine
            .validate_shape_sources(std::slice::from_ref(&spec), None, None, 64)
            .unwrap();
        assert_eq!(result["conforms"], false);
        assert_eq!(result["checked_shapes"], 1);
        assert!(result["violations"].as_u64().unwrap() > 0);
        fs::write(&spec.path, "<urn:changed> <urn:p> <urn:o> .").unwrap();
        assert!(
            engine
                .validate_shape_sources(&[spec], None, None, 64)
                .is_err()
        );
    }

    #[test]
    fn empty_shacl_selection_means_no_checks() {
        let dir = tempdir().unwrap();
        let (engine, _) = OntologyEngine::build(
            &[source(dir.path(), "data.ttl", "<urn:s> <urn:p> <urn:o> .")],
            "no-rules-v1",
            &[],
        )
        .unwrap();
        let selected = BTreeSet::new();
        let result = engine.validate(None, Some(&selected), None, 64).unwrap();
        assert_eq!(result["checked_shapes"], 0);
        assert_eq!(result["conforms"], true);
    }
}
