//! Full's process boundary owns budgets, terminal records and worker reaping.
use crate::full_artifact::{Stage, digest};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    fs::{self, File},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const CONTRACT: &str = "full-execution/v1";
static CANCELLED: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}

struct Signals(libc::sighandler_t, libc::sighandler_t);
impl Signals {
    fn install() -> Self {
        CANCELLED.store(false, Ordering::Relaxed);
        unsafe {
            Self(
                libc::signal(libc::SIGTERM, cancel as *const () as libc::sighandler_t),
                libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t),
            )
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGTERM, self.0);
            libc::signal(libc::SIGINT, self.1);
        }
    }
}

struct Worker<'a>(Child, &'a Cell<bool>);
impl Worker<'_> {
    fn reap(&mut self) -> Result<std::process::ExitStatus> {
        if self.0.try_wait()?.is_none() {
            self.0.kill()?;
        }
        let status = self.0.wait()?;
        self.1.set(true);
        Ok(status)
    }
}
impl Drop for Worker<'_> {
    fn drop(&mut self) {
        let _ = self.reap();
    }
}

fn failure(error: impl std::fmt::Display) -> Value {
    let text = error.to_string();
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        return value;
    }
    if let Some((code, message)) = text.split_once('|') {
        return json!({"code":code, "message":message});
    }
    json!({"code":"NATIVE_ENGINE_FAILED", "message":text})
}

pub(crate) fn terminal_status(reply: &Value) -> &'static str {
    match reply["error"]["code"]
        .as_str()
        .or(reply["envelope"]["execution"]["error"]["code"].as_str())
    {
        None => "COMPLETE",
        Some("CANCELLED") => "CANCELLED",
        Some("RESOURCE_LIMIT_EXCEEDED" | "BUDGET_EXHAUSTED" | "FIXPOINT_LIMIT_EXCEEDED") => {
            "BUDGET_EXHAUSTED"
        }
        Some("UNSUPPORTED_RULE_PROFILE" | "FORMAT_VERSION_UNSUPPORTED") => "UNSUPPORTED",
        _ => "FAILED",
    }
}

pub(crate) fn terminal_record(
    request: &Value,
    request_digest: &str,
    reply: &Value,
    reaped: bool,
) -> Value {
    json!({"schema_version":CONTRACT, "request_sha256":request_digest, "operation":request["operation"],
        "kernel":crate::identity::kernel(), "status":terminal_status(reply), "worker_reaped":reaped,
        "fixpoint_reached":reply["fixpoint_reached"], "integrity_checked":reply["integrity_checked"],
        "proof_coverage":reply.get("proof_coverage").cloned().unwrap_or(json!({"mode":"operation-defined"})),
        "artifact":reply["artifact"], "supervisor_pid":std::process::id()})
}

pub fn run_worker(request: &Path, result: &Path) -> Result<()> {
    let value: Value = serde_json::from_slice(&fs::read(request)?)?;
    let request_digest = value["_request_sha256"]
        .as_str()
        .context("supervisor request identity")?
        .to_owned();
    let mut reply = match if value["_api_request"].is_object() {
        crate::api::execute_worker(value.clone())
    } else if value["artifact"].is_object() {
        crate::full_artifact::compute(value.clone())
    } else {
        crate::full::execute(value.clone()).map_err(anyhow::Error::msg)
    } {
        Ok(value) => value,
        Err(error) => json!({"error":failure(error)}),
    };
    crate::full::add_usage(&mut reply)?;
    reply["execution"] = terminal_record(&value, &request_digest, &reply, false);
    write_atomic(result, &reply)?;
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("result parent")?)?;
    serde_json::to_writer(&mut temp, value)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    File::open(path.parent().context("result parent")?)?.sync_all()?;
    Ok(())
}

fn rss(pid: u32) -> u64 {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|s| {
                s.lines().find_map(|l| {
                    l.strip_prefix("VmRSS:")
                        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                })
            })
            .unwrap_or(0)
            * 1024
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|v| String::from_utf8(v.stdout).ok()?.trim().parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    }
}

fn disk(path: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(
            !entry.file_type().is_symlink(),
            "SOURCE_INVALID|scratch symlink"
        );
        if entry.file_type().is_file() {
            match entry.metadata() {
                Ok(metadata) => bytes = bytes.saturating_add(metadata.len()),
                Err(error)
                    if error
                        .io_error()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(bytes)
}

pub fn run_files(request_path: &Path, result_path: &Path) -> Result<()> {
    run_files_with(request_path, result_path, |reply, path| {
        write_atomic(path, reply)
    })
}

pub(crate) fn run_files_with(
    request_path: &Path,
    result_path: &Path,
    response: impl FnOnce(&Value, &Path) -> Result<()>,
) -> Result<()> {
    let _signals = Signals::install();
    let bytes = fs::read(request_path)?;
    let request: Value = serde_json::from_slice(&bytes)?;
    ensure!(request.is_object(), "request must be an object");
    let hash = digest(&bytes);
    let started = Instant::now();
    let mut stage = None;
    let mut peak_rss = 0u64;
    let mut peak_disk = 0u64;
    let mut worker_pid = None;
    let reaped = Cell::new(true);
    let memory = if request
        .get("memory_budget_bytes")
        .is_some_and(Value::is_null)
    {
        u64::MAX
    } else {
        request["memory_budget_bytes"]
            .as_u64()
            .unwrap_or(128 * 1024 * 1024)
    };
    let disk_budget = if request.get("disk_budget_bytes").is_some_and(Value::is_null) {
        u64::MAX
    } else {
        request["disk_budget_bytes"]
            .as_u64()
            .unwrap_or(4 * 1024 * 1024 * 1024)
    };
    let unlimited_time = request.get("timeout_seconds").is_some_and(Value::is_null);
    let seconds = if unlimited_time {
        f64::INFINITY
    } else {
        request["timeout_seconds"]
            .as_f64()
            .or(request["query_seconds"].as_f64())
            .or(request["arguments"]["query_seconds"].as_f64())
            .unwrap_or(300.0)
    };
    let budget_code = if matches!(
        request["operation"].as_str(),
        Some("compile" | "materialize")
    ) {
        "RESOURCE_LIMIT_EXCEEDED"
    } else {
        "BUDGET_EXHAUSTED"
    };
    let outcome = (|| -> Result<Value> {
        ensure!(
            memory > 0
                && disk_budget > 0
                && (unlimited_time || seconds.is_finite())
                && seconds > 0.0,
            "RESOURCE_LIMIT_EXCEEDED|invalid execution allowance"
        );
        let control = tempfile::Builder::new()
            .prefix("full-worker-")
            .tempdir_in(request_path.parent().context("request parent")?)?;
        let mut value = request.clone();
        value["_request_directory"] = json!(control.path());
        value["_request_sha256"] = json!(hash);
        if let Some(output) = value["artifact"]["output"].as_str() {
            stage = Some(Stage::reserve(Path::new(output))?);
            stage.as_mut().context("artifact reservation")?.own(&hash)?;
            value["artifact"]["stage"] =
                json!(stage.as_ref().context("artifact reservation")?.path());
            value["artifact"]["request_sha256"] = json!(hash);
        } else {
            ensure!(
                !value["artifact"].is_object(),
                "SOURCE_INVALID|artifact output required"
            );
        }
        let worker_request = control.path().join("request.json");
        let worker_result = control.path().join("result.json");
        fs::write(&worker_request, serde_json::to_vec(&value)?)?;
        let scratch = stage.as_ref().map_or(control.path(), Stage::path);
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["full-worker", "--request"])
            .arg(&worker_request)
            .arg("--result")
            .arg(&worker_result)
            .env("SQLITE_TMPDIR", scratch)
            .env("TMPDIR", scratch)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        // A killed supervisor cannot leave a live worker on Linux. Normal
        // cancellation still kills and waits explicitly before releasing scope.
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let parent = std::process::id();
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        libc::_exit(1);
                    }
                    Ok(())
                });
            }
        }
        let child = command.spawn()?;
        reaped.set(false);
        let mut worker = Worker(child, &reaped);
        worker_pid = Some(worker.0.id());
        if let Some(stage) = &stage {
            stage.finish_record(&json!({"request_sha256":hash, "stage":stage.path(), "status":"RUNNING", "worker_pid":worker.0.id(), "supervisor_pid":std::process::id()}))?;
        }
        let mut next_sample = Instant::now();
        loop {
            if CANCELLED.load(Ordering::Relaxed) {
                worker.reap()?;
                bail_cancelled()?;
            }
            let finished = worker.0.try_wait()?;
            if finished.is_none() && Instant::now() < next_sample {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            next_sample = Instant::now() + Duration::from_millis(50);
            peak_rss = peak_rss.max(rss(worker.0.id()).saturating_add(rss(std::process::id())));
            peak_disk = peak_disk
                .max(disk(control.path())? + stage.as_ref().map_or(Ok(0), |s| disk(s.path()))?);
            #[cfg(target_os = "linux")]
            if let Ok(entries) = fs::read_dir(format!("/proc/{}/fd", worker.0.id())) {
                let deleted = entries
                    .flatten()
                    .filter_map(|e| {
                        fs::read_link(e.path())
                            .ok()
                            .filter(|p| p.to_string_lossy().ends_with(" (deleted)"))
                            .and_then(|_| fs::metadata(e.path()).ok())
                    })
                    .map(|m| m.len())
                    .sum::<u64>();
                peak_disk = peak_disk.max(
                    disk(control.path())?
                        + stage.as_ref().map_or(Ok(0), |s| disk(s.path()))?
                        + deleted,
                );
            }
            ensure!(
                peak_rss <= memory
                    && peak_disk <= disk_budget
                    && started.elapsed().as_secs_f64() <= seconds,
                "{budget_code}|worker exceeded time, RSS or scratch allowance"
            );
            if let Some(exit) = finished {
                worker.0.wait()?;
                reaped.set(true);
                ensure!(
                    exit.success(),
                    "NATIVE_ENGINE_FAILED|worker exited with {exit}"
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let size = fs::metadata(&worker_result)
            .context("missing worker terminal record")?
            .len();
        ensure!(
            size <= memory,
            "{budget_code}|worker response exceeded memory allowance"
        );
        let mut reply: Value = serde_json::from_slice(&fs::read(&worker_result)?)?;
        let execution = &reply["execution"];
        ensure!(
            execution["schema_version"] == CONTRACT
                && execution["operation"] == request["operation"]
                && execution["request_sha256"] == hash
                && execution["kernel"] == crate::identity::kernel()
                && execution["status"] == terminal_status(&reply),
            "NATIVE_ENGINE_FAILED|missing or mismatched worker terminal record"
        );
        let reported = reply["metrics"]["peak_rss_bytes"]
            .as_u64()
            .context("missing worker RSS")?;
        peak_rss = peak_rss.max(reported.saturating_add(rss(std::process::id())));
        ensure!(
            peak_rss <= memory,
            "{budget_code}|worker exceeded peak RSS allowance"
        );
        if reply.get("error").is_none()
            && let Some(stage) = stage.as_mut()
        {
            reply["artifact"] = stage.publish()?;
        }
        Ok(reply)
    })();
    let mut reply = match outcome {
        Ok(value) => value,
        Err(error) => json!({"error":failure(error)}),
    };
    let mut supervisor_usage = json!({});
    crate::full::add_usage(&mut supervisor_usage)?;
    let supervisor_peak = supervisor_usage["metrics"]["peak_rss_bytes"]
        .as_u64()
        .unwrap_or(0);
    if reply.get("error").is_none() && supervisor_peak > memory {
        reply = json!({"error":{"code":budget_code,"message":"Full supervisor exceeded peak RSS allowance"}});
    }
    if CANCELLED.load(Ordering::Relaxed) {
        reply = json!({"error":{"code":"CANCELLED","message":"Full execution cancelled after worker reaping"}});
    }
    if reply.get("error").is_none() && started.elapsed().as_secs_f64() > seconds {
        reply =
            json!({"error":{"code":budget_code,"message":"Full finalization exceeded deadline"}});
    }
    reply["metrics"]["peak_rss_bytes"] = json!(peak_rss);
    reply["metrics"]["supervisor_peak_rss_bytes"] = json!(supervisor_peak);
    reply["metrics"]["peak_scratch_bytes"] = json!(peak_disk);
    reply["metrics"]["execution_seconds"] = json!(started.elapsed().as_secs_f64());
    reply["execution"] = terminal_record(&request, &hash, &reply, reaped.get());
    reply["execution"]["worker_pid"] = json!(worker_pid);
    if reply.get("error").is_none()
        && let Some(stage) = &stage
    {
        stage.finish_record(&reply["execution"])?;
    }
    // A consumer adapter must write its terminal response before committing publication.
    // Conversion or output I/O failure leaves Stage responsible for rollback.
    response(&reply, result_path)?;
    if reply.get("error").is_none()
        && let Some(stage) = stage.as_mut()
    {
        stage.commit();
    }
    Ok(())
}

fn bail_cancelled() -> Result<()> {
    anyhow::bail!("CANCELLED|Full execution cancelled")
}
