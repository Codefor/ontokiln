#[test]
fn full_artifacts_constraints_and_process_lifecycle() {
    let status = std::process::Command::new("python3")
        .arg("-B")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/full.py"))
        .arg(env!("CARGO_BIN_EXE_ontokiln"))
        .status()
        .unwrap();
    assert!(status.success());
}
