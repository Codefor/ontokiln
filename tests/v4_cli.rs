//! Pack v4 is the only supported CLI artifact format.
use serde_json::Value;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ontokiln"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn build_query_validate_and_inspect_use_only_pack_v4() {
    let root = tempdir().unwrap();
    let rdf = root.path().join("rdf");
    fs::create_dir(&rdf).unwrap();
    fs::write(rdf.join("schema.ttl"),
        "<urn:Dog> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Animal> .")
        .unwrap();
    fs::write(rdf.join("facts.nt"),
        "<urn:Fido> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Dog> .")
        .unwrap();
    let pack = root.path().join("graph.pack");
    let query = root.path().join("query.rq");
    fs::write(&query,"ASK WHERE { <urn:Fido> a <urn:Animal> }").unwrap();
    let source = rdf.to_str().unwrap();
    let target = pack.to_str().unwrap();
    let query_path = query.to_str().unwrap();

    let build = cli(&["build","--sources",source,"--pack",target]);
    assert!(build.status.success(), "{}",String::from_utf8_lossy(&build.stderr));
    let reply: Value = serde_json::from_slice(&build.stdout).unwrap();
    assert_eq!(reply["format"],"ontokiln-pack/v4");
    assert_eq!(reply["explicit_facts"],2);
    assert_eq!(reply["inferred_facts"],1);
    assert_eq!(reply["fixpoint_reached"],true);

    let answer = cli(&["query","--pack",target,"--query",query_path]);
    assert!(answer.status.success(), "{}",String::from_utf8_lossy(&answer.stderr));
    let answer: Value = serde_json::from_slice(&answer.stdout).unwrap();
    assert_eq!(answer["boolean"],true);

    let report = cli(&["validate","--pack",target]);
    assert!(report.status.success(), "{}",String::from_utf8_lossy(&report.stderr));
    let report: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["checked_shapes"],0);

    let inspect = cli(&["inspect","--pack",target]);
    assert!(inspect.status.success(), "{}",String::from_utf8_lossy(&inspect.stderr));
    let inspect: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(inspect["version"],4);
    assert_eq!(inspect["quads"],3);
}

#[test]
fn legacy_pack_format_is_rejected_without_fallback() {
    let dir = tempdir().unwrap();
    let path=dir.path().join("obsolete.pack");
    let mut bytes = b"ONTOKILN".to_vec();
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    fs::write(&path,bytes).unwrap();
    let inspect = cli(&["inspect","--pack",path.to_str().unwrap()]);
    assert!(!inspect.status.success());
    assert!(String::from_utf8_lossy(&inspect.stderr).contains("unsupported Pack format v3"));
}
