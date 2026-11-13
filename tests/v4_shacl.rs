//! Fixed SHACL regression oracles independent from the deleted SQLite engine.
//! They exercise Full-era semantics through the only supported Pack v4 path.
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::Source;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

fn source(root: &Path, name: &str, format: &str, rdf: &str) -> Source {
    let path = root.join(name);
    fs::write(&path, rdf).unwrap();
    Source {
        path,
        document_id: name.to_owned(),
        format: format.to_owned(),
        expected_sha256: format!("{:x}", Sha256::digest(rdf.as_bytes())),
        base_iri: None,
    }
}

fn build(root: &Path, data: &str, format: &str, profile: &str) -> OntologyEngine {
    OntologyEngine::build(&[source(root, "data.rdf", format, data)], profile, &[])
        .unwrap()
        .0
}

fn validate(root: &Path, engine: &OntologyEngine, shapes: &str) -> Value {
    let external = source(root, "shapes.ttl", "turtle", shapes);
    engine
        .validate_shape_sources(&[external], None, None, 64)
        .unwrap()
}

#[test]
fn shacl_sparql_constraint_and_sequence_path_consume_closure() {
    let root = tempdir().unwrap();
    let engine = build(
        root.path(),
        "<urn:a> <urn:edge> <urn:b> .\n<urn:b> <urn:name> \"Bob\"; <urn:blocked> true .",
        "turtle",
        "no-rules-v1",
    );
    let shapes = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        <urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
          sh:property [ sh:path (<urn:edge> <urn:name>); sh:hasValue "Bob" ];
          sh:sparql [
            sh:select "SELECT $this WHERE { $this <urn:edge> ?b . ?b <urn:blocked> true }"
          ] .
    "#;
    let report = validate(root.path(), &engine, shapes);
    assert_eq!(report["conforms"], false);
    assert_eq!(report["checked_shapes"], 1);
    let violations = report["report"]["violations"].as_array().unwrap();
    assert_eq!(violations.len(), 1);
    assert!(
        violations[0]["source_constraint_component"]
            .as_str()
            .unwrap()
            .ends_with("SPARQLConstraintComponent")
    );
}

#[test]
fn shacl_target_class_uses_materialized_rdfs_subclasses() {
    let root = tempdir().unwrap();
    let data = r#"
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        <urn:Dog> rdfs:subClassOf <urn:Animal> .
        <urn:Fido> a <urn:Dog> .
    "#;
    let engine = build(root.path(), data, "turtle", "rdfs-supported-v1");
    let shapes = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        <urn:Shape> a sh:NodeShape; sh:targetClass <urn:Animal>;
          sh:property [ sh:path <urn:name>; sh:minCount 1 ] .
    "#;
    let report = validate(root.path(), &engine, shapes);
    assert_eq!(report["conforms"], false);
    assert_eq!(report["checked_shapes"], 1);
    assert!(report["violations"].as_u64().unwrap() > 0);
    let covered = report["report"]["coverage"].as_array().unwrap();
    assert!(
        covered[0]["focus_nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node == "urn:Fido" || node == "<urn:Fido>")
    );
}

#[test]
fn shacl_data_graph_includes_named_graph_facts() {
    let root = tempdir().unwrap();
    let data = "<urn:g> { <urn:item> <urn:required> <urn:value> . }";
    let engine = build(root.path(), data, "trig", "no-rules-v1");
    let shapes = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        <urn:Shape> a sh:NodeShape; sh:targetNode <urn:item>;
          sh:property [ sh:path <urn:required>; sh:minCount 1 ] .
    "#;
    let report = validate(root.path(), &engine, shapes);
    assert_eq!(report["checked_shapes"], 1);
    assert_eq!(report["conforms"], true);
    assert_eq!(report["violations"], 0);
}
