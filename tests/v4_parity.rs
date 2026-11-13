//! Semantic differential tests against the previous Full implementation.
//! These are independent executions over identical input RDF, not tests that
//! copy expected output from the Pack v4 engine back into its own oracle.
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::Source;
use ontokiln_pack::v4::View;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn source(root: &Path, format: &str, body: &str) -> Source {
    let path = root.join("input.rdf");
    fs::write(&path, body).unwrap();
    Source {
        path,
        document_id: "input".into(),
        format: format.into(),
        expected_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
        base_iri: None,
    }
}
fn build_full(root: &Path, src: &Source, profile: &str) -> (Value, Value) {
    let root = root.join("full");
    fs::create_dir_all(&root).unwrap();
    let database = root.join("facts.sqlite");
    let rules = ontokiln::full::execute(json!({
        "operation":"rules", "profile":profile, "files":[]
    }))
    .unwrap();
    let compile = ontokiln::full::execute(json!({
        "operation":"compile",
        "database":database,
        "memory_budget_bytes":268435456u64,
        "disk_budget_bytes":1073741824u64,
        "max_term_bytes":1048576u64,
        "sources":[{
            "path":src.path, "document_id":src.document_id,
            "format":src.format, "base_iri":src.base_iri
        }],
        "rules":rules["rules"],
        "extract_property_chains":profile=="rdfs-property-chain-v1"
    }))
    .unwrap();
    let materialized = ontokiln::full::execute(json!({
        "operation":"materialize",
        "database":database,
        "memory_budget_bytes":268435456u64,
        "disk_budget_bytes":1073741824u64,
        "max_iterations":100u64,
        "max_derived_facts":10000u64,
        "max_intermediate_rows":100000u64,
        "plan":compile["plan"],
    }))
    .unwrap();
    (json!({"database":database,"dataset":root}), materialized)
}
fn full_query(paths: &Value, query: &str) -> Value {
    ontokiln::full::execute(json!({
        "operation":"query",
        "dataset":paths["database"],
        "query":query,
        "view":"closure",
        "max_rows":10000,
        "max_bytes":1048576u64,
        "query_seconds":null,
    }))
    .unwrap()
}
fn pack_query(engine: &OntologyEngine, query: &str) -> Value {
    serde_json::from_slice(&engine.query(query, View::Closure).unwrap()).unwrap()
}
fn compare_ask(paths: &Value, engine: &OntologyEngine, query: &str) {
    let full = full_query(paths, query);
    let pack = pack_query(engine, query);
    assert_eq!(full["boolean"], pack["boolean"], "ASK mismatch: {query}");
}
fn compare_count(paths: &Value, engine: &OntologyEngine, query: &str) {
    let full = full_query(paths, query);
    let pack = pack_query(engine, query);
    assert_eq!(
        full["items"][0]["n"]["value"], pack["results"]["bindings"][0]["n"]["value"],
        "COUNT mismatch: {query}"
    );
}

#[test]
fn rdfs_semantics_and_dataset_sourced_shacl_match_full() {
    let root = tempdir().unwrap();
    let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
    let rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    let sh = "http://www.w3.org/ns/shacl#";
    let ttl = format!(
        "<urn:Dog> <{rdfs}subClassOf> <urn:Animal> .\n\
         <urn:Animal> <{rdfs}subClassOf> <urn:Life> .\n\
         <urn:Fido> <{rdf}type> <urn:Dog> .\n\
         <urn:Shape> <{rdf}type> <{sh}NodeShape> .\n\
         <urn:Shape> <{sh}targetNode> <urn:Fido> .\n\
         <urn:Shape> <{sh}property> _:property .\n\
         _:property <{sh}path> <urn:missing> .\n\
         _:property <{sh}minCount> 1 .\n"
    );
    let src = source(root.path(), "turtle", &ttl);
    let (engine, stats) =
        OntologyEngine::build(std::slice::from_ref(&src), "rdfs-supported-v1", &[]).unwrap();
    let (paths, legacy) = build_full(root.path(), &src, "rdfs-supported-v1");
    assert_eq!(legacy["stats"]["explicit_facts"], stats.explicit_facts);
    assert_eq!(legacy["stats"]["inferred_facts"], stats.inferred_facts);
    compare_ask(&paths, &engine, "ASK WHERE { <urn:Fido> a <urn:Life> }");
    compare_count(
        &paths,
        &engine,
        "SELECT (COUNT(?t) AS ?n) WHERE { <urn:Fido> a ?t }",
    );

    let full = ontokiln::full::execute(json!({
        "operation":"validate",
        "dataset":paths["dataset"],
        "shapes":[],
        "shapes_from_dataset":true,
        "max_validation_depth":64
    }))
    .unwrap();
    let pack = engine.validate(None, None, None, 64).unwrap();
    assert_eq!(full["conforms"], pack["conforms"]);
    assert_eq!(full["checked_shapes"], pack["checked_shapes"]);
    assert_eq!(full["violations"], pack["violations"]);
}

#[test]
fn owl_property_chain_default_graph_and_named_graph_match_full() {
    let root = tempdir().unwrap();
    let rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    let owl = "http://www.w3.org/2002/07/owl#";
    let trig = format!(
        "<urn:ancestor> <{owl}propertyChainAxiom> _:a .\n\
         _:a <{rdf}first> <urn:parent> .\n\
         _:a <{rdf}rest> _:b .\n\
         _:b <{rdf}first> <urn:parent> .\n\
         _:b <{rdf}rest> <{rdf}nil> .\n\
         <urn:a> <urn:parent> <urn:b> .\n\
         <urn:b> <urn:parent> <urn:c> .\n\
         <urn:g> {{ <urn:x> <urn:parent> <urn:y> . <urn:y> <urn:parent> <urn:z> . }}\n"
    );
    let src = source(root.path(), "trig", &trig);
    let (engine, stats) =
        OntologyEngine::build(std::slice::from_ref(&src), "rdfs-property-chain-v1", &[]).unwrap();
    let (paths, legacy) = build_full(root.path(), &src, "rdfs-property-chain-v1");
    assert_eq!(legacy["stats"]["inferred_facts"], stats.inferred_facts);
    compare_ask(
        &paths,
        &engine,
        "ASK WHERE { <urn:a> <urn:ancestor> <urn:c> }",
    );
    compare_ask(
        &paths,
        &engine,
        "ASK WHERE { <urn:x> <urn:ancestor> <urn:z> }",
    );
    compare_ask(
        &paths,
        &engine,
        "ASK WHERE { GRAPH <urn:g> { <urn:x> <urn:parent> <urn:y> } }",
    );
}

#[test]
fn typed_parameters_rdf_bindings_and_values_match_full() {
    use ontokiln_reasoner::parameters::{Parameter, ValuesTable};
    use std::collections::BTreeMap;

    let root = tempdir().unwrap();
    let src = source(
        root.path(),
        "turtle",
        "<urn:a> <urn:p> <urn:o> .\n<urn:b> <urn:p> <urn:other> .\n",
    );
    let (engine, _) =
        OntologyEngine::build(std::slice::from_ref(&src), "no-rules-v1", &[]).unwrap();
    let (paths, _) = build_full(root.path(), &src, "no-rules-v1");

    let query = "SELECT ?s WHERE { ?s <urn:p> ?o FILTER(?o = ?wanted) } ORDER BY ?s";
    let bindings = BTreeMap::from([("wanted".into(), "<urn:o>".into())]);
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(query, View::Closure, &BTreeMap::new(), &bindings, &[])
            .unwrap(),
    )
    .unwrap();
    let expected = ontokiln::full::execute(json!({
        "operation":"query","dataset":paths["database"],"query":query,
        "view":"closure","rdf_bindings":bindings,"parameters":{},"values":[],
        "max_rows":10000,"max_bytes":1048576u64,"query_seconds":null
    }))
    .unwrap();
    assert_eq!(actual["head"]["vars"], json!(["s"]));
    assert_eq!(actual["results"]["bindings"], expected["items"]);

    let values_query = "SELECT ?s WHERE { VALUES (?s) {} ?s <urn:p> <urn:o> } ORDER BY ?s";
    let values = vec![ValuesTable {
        variables: vec!["s".into()],
        rows: vec![
            vec![Some(Parameter::Iri {
                value: "urn:a".into(),
            })],
            vec![Some(Parameter::Iri {
                value: "urn:a".into(),
            })],
        ],
    }];
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(
                values_query,
                View::Closure,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &values,
            )
            .unwrap(),
    )
    .unwrap();
    let expected = ontokiln::full::execute(json!({
        "operation":"query","dataset":paths["database"],"query":values_query,
        "view":"closure","values":values,"parameters":{},"rdf_bindings":{},
        "max_rows":10000,"max_bytes":1048576u64,"query_seconds":null
    }))
    .unwrap();
    assert_eq!(actual["results"]["bindings"], expected["items"]);
    assert_eq!(actual["results"]["bindings"].as_array().unwrap().len(), 2);

    let templated = "SELECT ?s WHERE { ?s <urn:p> <urn:ontokiln:param:target> }";
    let parameters = BTreeMap::from([(
        "target".into(),
        Parameter::Iri {
            value: "urn:o".into(),
        },
    )]);
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(templated, View::Closure, &parameters, &BTreeMap::new(), &[])
            .unwrap(),
    )
    .unwrap();
    let expected = ontokiln::full::execute(json!({
        "operation":"query","dataset":paths["database"],"query":templated,
        "view":"closure","parameters":parameters,"values":[],"rdf_bindings":{},
        "max_rows":10000,"max_bytes":1048576u64,"query_seconds":null
    }))
    .unwrap();
    assert_eq!(actual["results"]["bindings"], expected["items"]);
}
