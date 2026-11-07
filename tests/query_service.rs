// The child server, TCP disconnect and worker exit must be exercised across
// real process boundaries; dropping an in-process mock cannot verify them.
#[test]
fn query_lifecycle_and_versioned_assignment_chain() {
    let status = std::process::Command::new("python3")
        .arg("-B")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/query_service.py"
        ))
        .arg(env!("CARGO_BIN_EXE_ontokiln"))
        .status()
        .unwrap();
    assert!(status.success());
}
