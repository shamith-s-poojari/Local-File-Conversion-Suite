//! One supervised worker process (spec sections 7.1, 7.4 and 10.3).
//!
//! Everything the worker sends is untrusted. Every line is length-checked,
//! schema-checked and matched to the running job before it's acted on; any
//! violation kills the worker's process tree and fails with `E_PROTOCOL`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use converter_jobstore::{JobFailure, JobId};
use converter_protocol::{Codec, Direction, Secrets, MAX_LINE_BYTES, PROTOCOL_VERSION};
use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::tree::{prepare, ProcessTree};

/// Size of the in-memory stderr ring buffer. Never written to disk: it may
/// hold document text.
pub const STDERR_BUFFER_BYTES: usize = 256 * 1024;

/// More messages than this in one second kills the worker.
pub const MAX_MESSAGES_PER_SECOND: u32 = 1000;

/// Exit code of a launcher process that couldn't apply the sandbox. A
/// worker that exits with it before `hello` fails the job with
/// `E_SANDBOX_UNAVAILABLE` rather than `E_INSTALL_CORRUPT`. (A worker could
/// fake it, but that only changes which error the user sees.)
pub const SANDBOX_SETUP_FAILED_EXIT: i32 = 97;

/// What a launcher needs to start a worker.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub adapter_id: String,
    pub job_id: JobId,
    /// The job directory, which is the worker's working directory.
    pub job_dir: PathBuf,
    /// Memory the sandbox must enforce, from admission. `None` before the
    /// job is admitted (validate and estimate), when the sandbox applies its
    /// default ceiling.
    pub memory_limit: Option<u64>,
}

/// Builds the command that runs a worker inside its sandbox.
///
/// Implemented by the sandbox (T09, T10). There is no unsandboxed launcher
/// in the product: if the sandbox can't be applied, `command` returns an
/// error and the job fails with `E_SANDBOX_UNAVAILABLE` (spec section 5.2).
pub trait Launcher: Send + Sync {
    fn command(&self, spec: &LaunchSpec) -> Result<Command, String>;
}

/// Timing rules. Defaults follow the spec.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// `hello` must arrive within this long of launch.
    pub hello: Duration,
    /// After `cancel`, wait this long before killing.
    pub cancel_grace: Duration,
    /// After closing stdin, wait this long for the worker to exit.
    pub exit_grace: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            hello: Duration::from_secs(10),
            cancel_grace: Duration::from_secs(2),
            exit_grace: Duration::from_secs(1),
        }
    }
}

/// Watchdog limits for one request.
#[derive(Debug, Clone, Copy)]
pub struct Watchdog {
    /// `timeoutBaseS + timeoutSPerMb × input MB`.
    pub total: Duration,
    /// `noProgressS`: longest silence allowed.
    pub silence: Duration,
}

impl Watchdog {
    /// Limits from manifest values and the total input size.
    pub fn from_limits(
        timeout_base_s: u32,
        timeout_s_per_mb: f64,
        no_progress_s: u32,
        input_bytes: u64,
    ) -> Self {
        let mb = input_bytes as f64 / (1024.0 * 1024.0);
        let total = f64::from(timeout_base_s) + timeout_s_per_mb.max(0.0) * mb;
        Watchdog {
            total: Duration::try_from_secs_f64(total).unwrap_or(Duration::MAX),
            silence: Duration::from_secs(u64::from(no_progress_s)),
        }
    }
}

/// Shared cancel flag for one job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A non-final message from a worker during a request.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerEvent {
    Stage {
        name: String,
        index: u64,
        of: u64,
    },
    Progress {
        stage: String,
        done: u64,
        total: Option<u64>,
    },
    Warning {
        code: String,
        detail: Option<Value>,
    },
}

/// Core requests the supervisor sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Inspect,
    Validate,
    Estimate,
    Execute,
    Render,
    PageText,
}

impl RequestKind {
    fn wire(self) -> &'static str {
        match self {
            RequestKind::Inspect => "inspect",
            RequestKind::Validate => "validate",
            RequestKind::Estimate => "estimate",
            RequestKind::Execute => "execute",
            RequestKind::Render => "render",
            RequestKind::PageText => "pageText",
        }
    }

    fn result(self) -> &'static str {
        match self {
            RequestKind::Inspect => "inspectResult",
            RequestKind::Validate => "validateResult",
            RequestKind::Estimate => "estimateResult",
            RequestKind::Execute => "executeResult",
            RequestKind::Render => "renderResult",
            RequestKind::PageText => "pageTextResult",
        }
    }
}

pub(crate) fn failure(code: &str) -> JobFailure {
    JobFailure {
        code: code.to_owned(),
        retryable: retryable(code),
    }
}

/// Whether a catalog error is retryable (spec section 13.1).
pub fn retryable(code: &str) -> bool {
    matches!(
        code,
        "E_PASSWORD_REQUIRED"
            | "E_PASSWORD_WRONG"
            | "E_RESOURCE_MEMORY"
            | "E_RESOURCE_DISK"
            | "E_TIMEOUT"
            | "E_INTERNAL"
    )
}

enum Incoming {
    Line(Vec<u8>),
    TooLong,
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancelled,
    TimedOut,
}

impl Stop {
    fn code(self) -> &'static str {
        match self {
            Stop::Cancelled => "E_CANCELLED",
            Stop::TimedOut => "E_TIMEOUT",
        }
    }
}

/// A running worker. Dropping it kills the process tree.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    tree: ProcessTree,
    lines: Receiver<Incoming>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    codec: Arc<Codec>,
    job_id: JobId,
    timing: Timing,
    rate_window: Instant,
    rate_count: u32,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("job_id", &self.job_id)
            .field("pid", &self.child.id())
            .finish_non_exhaustive()
    }
}

impl Worker {
    /// Launches a worker and waits for a valid `hello`.
    pub fn spawn(
        launcher: &dyn Launcher,
        spec: &LaunchSpec,
        codec: Arc<Codec>,
        timing: Timing,
    ) -> Result<Worker, JobFailure> {
        let mut cmd = launcher
            .command(spec)
            .map_err(|_| failure("E_SANDBOX_UNAVAILABLE"))?;
        prepare(&mut cmd);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|_| failure("E_INSTALL_CORRUPT"))?;
        let tree = match ProcessTree::attach(&child) {
            Ok(t) => t,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failure("E_SANDBOX_UNAVAILABLE"));
            }
        };

        let (tx, lines) = mpsc::channel();
        let stdout = child.stdout.take().expect("stdout is piped");
        thread::spawn(move || read_lines(stdout, tx));
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let err_pipe = child.stderr.take().expect("stderr is piped");
        let sink = stderr.clone();
        thread::spawn(move || drain_stderr(err_pipe, sink));

        let stdin = child.stdin.take();
        let mut worker = Worker {
            child,
            stdin,
            tree,
            lines,
            stderr,
            codec,
            job_id: spec.job_id.clone(),
            timing,
            rate_window: Instant::now(),
            rate_count: 0,
        };
        worker.handshake(&spec.adapter_id)?;
        Ok(worker)
    }

    fn handshake(&mut self, adapter_id: &str) -> Result<(), JobFailure> {
        let line = match self.lines.recv_timeout(self.timing.hello) {
            Ok(Incoming::Line(l)) => l,
            Ok(Incoming::TooLong) => return Err(self.kill_with("E_PROTOCOL")),
            Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                let code = if self.exited_with(SANDBOX_SETUP_FAILED_EXIT) {
                    "E_SANDBOX_UNAVAILABLE"
                } else {
                    "E_INSTALL_CORRUPT"
                };
                return Err(self.kill_with(code));
            }
            Err(RecvTimeoutError::Timeout) => return Err(self.kill_with("E_TIMEOUT")),
        };
        let Ok(msg) = self.codec.validate(Direction::Worker, &line) else {
            return Err(self.kill_with("E_PROTOCOL"));
        };
        if msg["type"] != "hello" || msg["jobId"] != self.job_id.as_str() {
            return Err(self.kill_with("E_PROTOCOL"));
        }
        // Spec 7.5: a version mismatch means a broken install.
        let version = u64::from(PROTOCOL_VERSION);
        if msg["v"] != version
            || msg["protocolVersion"] != version
            || msg["adapterId"] != adapter_id
        {
            return Err(self.kill_with("E_INSTALL_CORRUPT"));
        }
        Ok(())
    }

    /// Sends one request and supervises it until its result.
    ///
    /// `body` holds the request's fields other than `v`, `type`, `jobId`
    /// and `secrets`. Secrets are serialized straight into a zeroizing
    /// buffer and never pass through a `Value`.
    ///
    /// The watchdog sends `cancel` when the request runs longer than
    /// `watchdog.total` or the worker is silent for `watchdog.silence`, waits
    /// the cancel grace, then kills the tree (`E_TIMEOUT`). Cancelling `token`
    /// does the same with `E_CANCELLED`.
    pub fn request(
        &mut self,
        kind: RequestKind,
        body: Value,
        secrets: Option<&Secrets>,
        watchdog: Watchdog,
        token: &CancelToken,
        on_event: &mut dyn FnMut(WorkerEvent),
    ) -> Result<Value, JobFailure> {
        let line = encode(kind.wire(), &self.job_id, body, secrets);
        debug_assert!(
            self.codec.validate(Direction::Core, &line).is_ok(),
            "the core built an invalid {} message",
            kind.wire()
        );
        if self.send(&line).is_err() {
            return Err(self.kill_with("E_INTERNAL"));
        }
        drop(line);

        let start = Instant::now();
        let mut last_message = start;
        let mut stop: Option<(Stop, Instant)> = None;
        loop {
            let now = Instant::now();
            if stop.is_none() {
                let why = if token.is_cancelled() {
                    Some(Stop::Cancelled)
                } else if now.duration_since(start) >= watchdog.total
                    || now.duration_since(last_message) >= watchdog.silence
                {
                    Some(Stop::TimedOut)
                } else {
                    None
                };
                if let Some(why) = why {
                    let _ = self.send(&encode("cancel", &self.job_id, Value::Null, None));
                    stop = Some((why, now + self.timing.cancel_grace));
                }
            }
            let wait = match stop {
                Some((why, deadline)) => {
                    if now >= deadline {
                        return Err(self.kill_with(why.code()));
                    }
                    deadline - now
                }
                None => {
                    let total_left = watchdog.total.saturating_sub(now.duration_since(start));
                    let silence_left = watchdog
                        .silence
                        .saturating_sub(now.duration_since(last_message));
                    total_left.min(silence_left)
                }
            };
            // Wake regularly to notice a cancel.
            let wait = wait.min(Duration::from_millis(50));

            let line = match self.lines.recv_timeout(wait) {
                Ok(Incoming::Line(l)) => l,
                Err(RecvTimeoutError::Timeout) => continue,
                Ok(Incoming::TooLong) => return Err(self.kill_with("E_PROTOCOL")),
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    // The worker died without answering.
                    let code = stop.map_or("E_INTERNAL", |(why, _)| why.code());
                    return Err(self.kill_with(code));
                }
            };
            last_message = Instant::now();
            if !self.within_rate(last_message) {
                return Err(self.kill_with("E_PROTOCOL"));
            }
            let Ok(msg) = self.codec.validate(Direction::Worker, &line) else {
                return Err(self.kill_with("E_PROTOCOL"));
            };
            if msg["jobId"] != self.job_id.as_str() {
                return Err(self.kill_with("E_PROTOCOL"));
            }
            match msg["type"].as_str().unwrap_or_default() {
                "stage" => on_event(WorkerEvent::Stage {
                    name: str_field(&msg, "name"),
                    index: msg["index"].as_u64().unwrap_or(0),
                    of: msg["of"].as_u64().unwrap_or(0),
                }),
                "progress" => on_event(WorkerEvent::Progress {
                    stage: str_field(&msg, "stage"),
                    done: msg["done"].as_u64().unwrap_or(0),
                    total: msg["total"].as_u64(),
                }),
                "warning" => on_event(WorkerEvent::Warning {
                    code: str_field(&msg, "code"),
                    detail: msg.get("detail").cloned(),
                }),
                "error" => {
                    if let Some((why, _)) = stop {
                        return Err(failure(why.code()));
                    }
                    let code = str_field(&msg, "code");
                    return Err(JobFailure {
                        retryable: msg["retryable"].as_bool().unwrap_or(false),
                        code,
                    });
                }
                t if t == kind.result() => {
                    // A result that races a cancel or timeout doesn't count.
                    if let Some((why, _)) = stop {
                        return Err(self.kill_with(why.code()));
                    }
                    return Ok(msg);
                }
                // A second hello, or the wrong kind of result.
                _ => return Err(self.kill_with("E_PROTOCOL")),
            }
        }
    }

    /// Closes stdin, gives the worker the exit grace to leave, then kills the
    /// tree.
    pub fn shutdown(mut self) {
        self.stdin = None;
        let deadline = Instant::now() + self.timing.exit_grace;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Drop kills the tree and reaps the child.
    }

    /// The last [`STDERR_BUFFER_BYTES`] of stderr, for diagnostics shown to
    /// the user on request. Never log or persist it.
    pub fn stderr_tail(&self) -> Vec<u8> {
        let buf = self.stderr.lock().unwrap_or_else(|e| e.into_inner());
        buf.iter().copied().collect()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, line: &[u8]) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("stdin closed"))?;
        stdin.write_all(line)?;
        stdin.flush()
    }

    fn within_rate(&mut self, now: Instant) -> bool {
        if now.duration_since(self.rate_window) >= Duration::from_secs(1) {
            self.rate_window = now;
            self.rate_count = 0;
        }
        self.rate_count += 1;
        self.rate_count <= MAX_MESSAGES_PER_SECOND
    }

    /// Waits briefly for the process to exit and checks its exit code.
    fn exited_with(&mut self, expected: i32) -> bool {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code() == Some(expected),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => return false,
            }
        }
    }

    fn kill_with(&mut self, code: &str) -> JobFailure {
        self.tree.kill();
        let _ = self.child.kill();
        failure(code)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stdin = None;
        self.tree.kill();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn str_field(msg: &Value, key: &str) -> String {
    msg[key].as_str().unwrap_or_default().to_owned()
}

#[derive(Serialize)]
struct Envelope<'a> {
    v: u32,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(rename = "jobId")]
    job_id: &'a str,
    #[serde(flatten)]
    body: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    secrets: Option<&'a Secrets>,
}

fn encode(
    kind: &str,
    job_id: &JobId,
    body: Value,
    secrets: Option<&Secrets>,
) -> Zeroizing<Vec<u8>> {
    let body = match body {
        Value::Object(_) => body,
        _ => Value::Object(Default::default()),
    };
    let mut line = Zeroizing::new(Vec::with_capacity(256));
    serde_json::to_writer(
        &mut *line,
        &Envelope {
            v: PROTOCOL_VERSION,
            kind,
            job_id: job_id.as_str(),
            body,
            secrets,
        },
    )
    .expect("core messages always serialize");
    line.push(b'\n');
    line
}

/// Splits stdout into lines of at most [`MAX_LINE_BYTES`] (plus `\r\n`),
/// without ever buffering more than that.
fn read_lines(stdout: impl Read, tx: Sender<Incoming>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut line = Vec::new();
    loop {
        let buf = match reader.fill_buf() {
            Ok(b) if !b.is_empty() => b,
            _ => {
                let _ = tx.send(Incoming::Closed);
                return;
            }
        };
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                line.extend_from_slice(&buf[..i]);
                reader.consume(i + 1);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.len() > MAX_LINE_BYTES {
                    let _ = tx.send(Incoming::TooLong);
                    return;
                }
                if tx.send(Incoming::Line(std::mem::take(&mut line))).is_err() {
                    return;
                }
            }
            None => {
                let n = buf.len();
                line.extend_from_slice(buf);
                reader.consume(n);
                if line.len() > MAX_LINE_BYTES + 1 {
                    let _ = tx.send(Incoming::TooLong);
                    return;
                }
            }
        }
    }
}

fn drain_stderr(mut pipe: impl Read, sink: Arc<Mutex<VecDeque<u8>>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut buf = sink.lock().unwrap_or_else(|e| e.into_inner());
                buf.extend(&chunk[..n]);
                let excess = buf.len().saturating_sub(STDERR_BUFFER_BYTES);
                buf.drain(..excess);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_reader_limits_and_splits() {
        let (tx, rx) = mpsc::channel();
        let data = b"{\"a\":1}\r\n{\"b\":2}\npartial".to_vec();
        read_lines(&data[..], tx);
        let got: Vec<_> = rx.iter().collect();
        assert!(matches!(&got[0], Incoming::Line(l) if l == b"{\"a\":1}"));
        assert!(matches!(&got[1], Incoming::Line(l) if l == b"{\"b\":2}"));
        assert!(matches!(got[2], Incoming::Closed));

        let (tx, rx) = mpsc::channel();
        let long = vec![b'x'; MAX_LINE_BYTES + 10];
        read_lines(&long[..], tx);
        assert!(matches!(rx.recv().unwrap(), Incoming::TooLong));
    }

    #[test]
    fn exact_limit_is_allowed() {
        let (tx, rx) = mpsc::channel();
        let mut data = vec![b'x'; MAX_LINE_BYTES];
        data.extend_from_slice(b"\r\n");
        read_lines(&data[..], tx);
        assert!(matches!(rx.recv().unwrap(), Incoming::Line(l) if l.len() == MAX_LINE_BYTES));
    }

    #[test]
    fn encode_includes_secrets_and_validates() {
        let codec = Codec::new();
        let id = JobId::parse("j_1").unwrap();
        let secrets: Secrets =
            serde_json::from_value(serde_json::json!({"password": "pw"})).unwrap();
        let line = encode(
            "inspect",
            &id,
            serde_json::json!({"input": "in/0.pdf"}),
            Some(&secrets),
        );
        codec.validate(Direction::Core, &line).unwrap();
        assert!(String::from_utf8_lossy(&line).contains("\"password\":\"pw\""));
        codec
            .validate(Direction::Core, &encode("cancel", &id, Value::Null, None))
            .unwrap();
    }

    #[test]
    fn watchdog_formula() {
        let w = Watchdog::from_limits(60, 1.0, 120, 10 * 1024 * 1024);
        assert_eq!(w.total, Duration::from_secs(70));
        assert_eq!(w.silence, Duration::from_secs(120));
        let w = Watchdog::from_limits(60, f64::INFINITY, 1, 1);
        assert_eq!(w.total, Duration::MAX);
    }

    #[test]
    fn stderr_ring_keeps_the_tail() {
        let sink = Arc::new(Mutex::new(VecDeque::new()));
        let data: Vec<u8> = (0..STDERR_BUFFER_BYTES + 5000)
            .map(|i| (i % 251) as u8)
            .collect();
        drain_stderr(&data[..], sink.clone());
        let buf = sink.lock().unwrap();
        assert_eq!(buf.len(), STDERR_BUFFER_BYTES);
        assert_eq!(buf.back(), data.last());
    }
}
