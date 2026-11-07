//! Supervise the entire evaluation, including CPU-only work and backpressure.
use crate::{
    ApiError, App, allocation, error,
    worker::{self, Job},
};
use axum::{
    body::{Body, Bytes},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use serde_json::json;
use std::{
    collections::HashMap,
    io,
    os::fd::AsRawFd,
    process::Stdio,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Registry {
    next: AtomicU64,
    pending: Mutex<HashMap<String, Option<oneshot::Sender<()>>>>,
}
impl Registry {
    pub fn cancel(&self, id: &str) -> bool {
        self.pending
            .lock()
            .unwrap()
            .get_mut(id)
            .and_then(Option::take)
            .is_some_and(|tx| tx.send(()).is_ok())
    }
}
type Reply = Result<Bytes, ApiError>;
fn failure(message: impl Into<String>) -> ApiError {
    error(StatusCode::UNPROCESSABLE_ENTITY, message)
}
pub async fn run(
    app: App,
    job: Job,
    requested_id: Option<String>,
    heap: usize,
    deadline: Instant,
    media: &'static str,
) -> Result<Response, ApiError> {
    let registry = app.jobs.clone();
    let id = requested_id
        .unwrap_or_else(|| format!("q-{}", registry.next.fetch_add(1, Ordering::Relaxed)));
    if id.is_empty()
        || id.len() > 100
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request_id"));
    }
    let permit = app.slot()?;
    let (cancel, mut cancelled) = oneshot::channel();
    {
        let mut jobs = registry.pending.lock().unwrap();
        if jobs.contains_key(&id) {
            return Err(error(StatusCode::CONFLICT, "request_id already active"));
        }
        jobs.insert(id.clone(), Some(cancel));
    }
    let (sender, mut receiver) = mpsc::channel::<Reply>(2);
    let jsonl = job.jsonl;
    let key = id.clone();
    tokio::spawn(async move {
        let result = supervise(&app, &job, heap, deadline, &mut cancelled, &sender).await;
        registry.pending.lock().unwrap().remove(&key);
        drop(permit);
        let _ = sender.send(result.map(Bytes::from)).await;
    });
    let first = receiver
        .recv()
        .await
        .ok_or_else(|| failure("query worker closed"))??;
    let body = if jsonl {
        Body::from_stream(stream::unfold(
            (Some(first), receiver),
            |(first, mut rx)| async move {
                let bytes = if let Some(b) = first {
                    b
                } else {
                    match rx.recv().await? {
                        Ok(b) => b,
                        Err((code, msg)) => Bytes::from(format!(
                            "{}\n",
                            json!({"error":msg.0.error,"status":code.as_u16()})
                        )),
                    }
                };
                Some((Ok::<_, io::Error>(bytes), (None, rx)))
            },
        ))
    } else {
        Body::from(first)
    };
    Ok((
        [(header::CONTENT_TYPE.as_str(), media), ("x-query-id", &id)],
        body,
    )
        .into_response())
}
async fn read_output(
    pipe: tokio::process::ChildStdout,
    jsonl: bool,
    sender: &mpsc::Sender<Reply>,
) -> io::Result<Vec<u8>> {
    let mut reader = BufReader::new(pipe);
    let mut buffer = Vec::new();
    if !jsonl {
        reader.read_to_end(&mut buffer).await?;
        return Ok(buffer);
    }
    loop {
        let n = reader.read_until(b'\n', &mut buffer).await?;
        if n == 0 || !buffer.ends_with(b"\n") {
            return Err(io::Error::other("worker closed before end record"));
        }
        // Publish the terminal record only after wait() confirms success.
        if buffer.starts_with(b"{\"end\":true") {
            return Ok(buffer);
        }
        sender
            .send(Ok(Bytes::from(std::mem::take(&mut buffer))))
            .await
            .map_err(|_| io::Error::other("client disconnected"))?;
    }
}

async fn supervise(
    app: &App,
    job: &Job,
    heap: usize,
    deadline: Instant,
    cancel: &mut oneshot::Receiver<()>,
    sender: &mpsc::Sender<Reply>,
) -> Result<Vec<u8>, ApiError> {
    let fd = app
        .engine
        .snapshot_file()
        .expect("server snapshot")
        .as_raw_fd();
    let mut cmd = Command::new(&app.worker_binary);
    cmd.arg("query-worker")
        .arg(fd.to_string())
        .arg(heap.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: fcntl is async-signal-safe. The parent holds this FD alive.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let work = async {
        let request = serde_json::to_vec(job).map_err(|e| failure(e.to_string()))?;
        let write = async move {
            input.write_all(&request).await?;
            input.shutdown().await
        };
        // Keep reading to EOF so even a long diagnostic cannot block wait().
        let diagnostic = async {
            let mut message = Vec::new();
            let mut input = BufReader::new(stderr);
            (&mut input)
                .take(16 * 1024)
                .read_to_end(&mut message)
                .await?;
            tokio::io::copy(&mut input, &mut tokio::io::sink()).await?;
            Ok::<_, io::Error>(String::from_utf8_lossy(&message).into_owned())
        };
        let (written, bytes, msg) =
            tokio::join!(write, read_output(output, job.jsonl, sender), diagnostic);
        let status = child.wait().await.map_err(|e| failure(e.to_string()))?;
        if status.code() == Some(allocation::MEMORY_EXIT) {
            return Err(failure("query heap budget exceeded"));
        }
        if status.code() == Some(worker::INVALID_EXIT) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                msg.unwrap_or_default().trim().to_owned(),
            ));
        }
        if !status.success() {
            return Err(failure(
                msg.unwrap_or_default()
                    .trim()
                    .trim_start_matches("Error: ")
                    .to_owned(),
            ));
        }
        written.map_err(|e| failure(e.to_string()))?;
        bytes.map_err(|e| failure(e.to_string()))
    };
    let result = tokio::select! {
        biased;
        _=sender.closed()=>Err(failure("client disconnected")),
        _=cancel=>Err(error(StatusCode::CONFLICT,"query cancelled")),
        _=tokio::time::sleep_until(deadline)=>Err(error(StatusCode::REQUEST_TIMEOUT,"query deadline exceeded")),
        result=work=>result,
    };
    if result.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    result
}

pub fn timeout(ms: u64) -> Result<Instant, ApiError> {
    if !(1..=5000).contains(&ms) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "deadline_ms must be 1..5000",
        ));
    }
    Ok(Instant::now() + Duration::from_millis(ms))
}
pub fn heap(bytes: Option<usize>) -> Result<usize, ApiError> {
    let limit = bytes.unwrap_or(worker::MAX_HEAP);
    if !(1..=worker::MAX_HEAP).contains(&limit) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid memory_bytes"));
    }
    Ok(limit)
}
