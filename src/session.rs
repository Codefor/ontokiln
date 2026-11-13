//! A process-scoped, read-only transport over the existing API v1 contract.
//!
//! Mounting fixes one verified artifact before READY. Each line is subsequently
//! executed in the ordinary budgeted worker, inheriting the private snapshot FD.
use anyhow::Result;
use serde_json::json;
use std::{
    fs,
    io::{BufRead, Write},
    path::Path,
    time::Instant,
};

pub const CONTRACT: &str = "ontokiln-session/v1";

/// Execute serial API requests until EOF or cancellation. The caller selects
/// the artifact at process startup; requests cannot mount or switch datasets.
pub fn run(dataset: &Path, snapshot_bytes: u64) -> Result<()> {
    let started = Instant::now();
    let snapshot = crate::full_snapshot::FullSnapshot::mount(dataset, snapshot_bytes)?;
    let control = tempfile::Builder::new()
        .prefix("ontokiln-session-")
        .tempdir()?;
    let request = control.path().join("request.json");
    let result = control.path().join("result.json");
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(
        &mut output,
        &json!({
            "contract": CONTRACT,
            "status": "READY",
            "operations": ["capabilities", "prepare_query", "query", "validate"],
            "dataset": snapshot.source_path(),
            "content_digest": snapshot.manifest()["content_digest"],
            "manifest_sha256": snapshot.manifest_sha256(),
            "kernel": crate::identity::kernel(),
            "mount": {
                "elapsed_seconds": started.elapsed().as_secs_f64(),
                "snapshot_bytes": snapshot.database_bytes(),
                "copied_bytes": snapshot.database_bytes(),
            },
        }),
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    let mut input = std::io::stdin().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        fs::write(&request, &line)?;
        crate::api::run_files_on_snapshot(&request, &result, Some(&snapshot))?;
        output.write_all(&fs::read(&result)?)?;
        output.write_all(b"\n")?;
        output.flush()?;
        // The worker has already been killed and waited before returning its
        // CANCELLED envelope. Do not block waiting for another line afterward.
        if crate::full_execution::cancelled() {
            break;
        }
    }
    Ok(())
}
