//! Exercise the public JSON API v1 against the in-memory Pack v4 adapter.
//! Each request runs in the actual supervised ontokiln binary process.
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::Source;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn query(root: &Path, operation: Value, request_id: &str) -> Value {
    let req = root.join(format!("{request_id}.json"));
    let result = root.join(format!("{request_id}-result.json"));
    fs::write(
        &req,
        serde_json::to_vec(&json!({
            "contract":"ontokiln-api/v1","request_id":request_id,"operation":operation,
            "limits":{"memory_bytes":268435456,"scratch_bytes":1073741824,
                "timeout_ms":30000,"max_rows":10000,"max_bytes":1048576}
        }))
        .unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ontokiln"))
        .args(["api", "--request"])
        .arg(&req)
        .args(["--result"])
        .arg(&result)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "API failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&fs::read(&result).unwrap()).unwrap()
}

#[test]
fn v4_pack_read_api_preserves_query_and_shacl_contract() {
    let root = tempdir().unwrap();
    let turtle = concat!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n",
        "<urn:a> <urn:p> <urn:o> .\n",
        "<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;\n",
        "    sh:property [sh:path <urn:missing>; sh:minCount 1] .\n"
    );
    let src = root.path().join("data.ttl");
    fs::write(&src, turtle).unwrap();
    let source = Source {
        path: src,
        document_id: "data".into(),
        format: "turtle".into(),
        expected_sha256: format!("{:x}", Sha256::digest(turtle.as_bytes())),
        base_iri: None,
    };
    let (engine, _) = OntologyEngine::build(&[source], "no-rules-v1", &[]).unwrap();
    let pack = root.path().join("ontology.pack");
    engine.save_pack(&pack).unwrap();

    let capabilities = query(
        root.path(),
        json!({
            "kind":"capabilities","dataset":pack
        }),
        "capabilities",
    );
    assert_eq!(capabilities["status"], "COMPLETE");
    assert_eq!(
        capabilities["result"]["dataset"]["semantics"],
        "rdf-dataset-union/v1"
    );
    let caps = capabilities["result"]["dataset"]["capabilities"]
        .as_array()
        .unwrap();
    assert!(caps.contains(&json!("shacl/v1")));
    assert!(caps.contains(&json!("rdf-bindings/v1")));
    assert!(!caps.contains(&json!("derived-query-data/v1")));

    let q = query(
        root.path(),
        json!({
            "kind":"query","dataset":pack,
            "query":"SELECT ?s WHERE { ?s <urn:p> ?o FILTER(?o=?wanted) }",
            "rdf_bindings":{"wanted":"<urn:o>"},"view":"closure",
            "require":["rdf-bindings/v1"]
        }),
        "bound-select",
    );
    assert_eq!(q["status"], "COMPLETE");
    assert_eq!(q["result"]["query_type"], "SELECT");
    assert_eq!(q["result"]["variables"], json!(["s"]));
    assert_eq!(q["result"]["items"][0]["s"]["value"], "urn:a");

    let report = query(
        root.path(),
        json!({
            "kind":"validate","dataset":pack,"shapes_from_dataset":true
        }),
        "validate",
    );
    assert_eq!(report["status"], "COMPLETE");
    assert_eq!(report["result"]["conforms"], false);
    assert_eq!(report["result"]["checked_shapes"], 1);
    assert!(report["result"]["violations"].as_u64().unwrap() > 0);

    let unsupported = query(
        root.path(),
        json!({
            "kind":"query","dataset":pack,
            "query":"ASK { <urn:a> <urn:p> <urn:o> }",
            "derived_queries":[
                "CONSTRUCT { <urn:x> <urn:p> <urn:o> } WHERE { <urn:a> <urn:p> <urn:o> }"
            ]
        }),
        "derived-unsupported",
    );
    assert_eq!(unsupported["status"], "UNSUPPORTED");
    assert_eq!(unsupported["error"]["code"], "UNSUPPORTED_CAPABILITY");
}
