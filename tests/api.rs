#[test]
fn consumer_contract_on_ten_thousand_rdf_triples() {
    let status = std::process::Command::new("python3")
        .arg("-B")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/api.py"))
        .arg(env!("CARGO_BIN_EXE_ontokiln"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn mounted_sessions_preserve_contract_and_worker_isolation() {
    let status = std::process::Command::new("python3")
        .arg("-B")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/runtime/session.py"
        ))
        .arg(env!("CARGO_BIN_EXE_ontokiln"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn shacl_indexed_targets_and_paths_preserve_semantics() {
    let status = std::process::Command::new("python3")
        .arg("-B")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/shacl/validation.py"
        ))
        .arg(env!("CARGO_BIN_EXE_ontokiln"))
        .status()
        .unwrap();
    assert!(status.success());
}
