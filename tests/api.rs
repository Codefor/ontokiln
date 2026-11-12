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
