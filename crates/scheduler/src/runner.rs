//! Runs jobs end to end: validate, estimate, admit, execute, verify
//! (spec section 5.1), keeping the job store authoritative.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use converter_jobstore::{JobFailure, JobId, JobState, JobStore, Progress, Warning};
use converter_protocol::{Codec, Secrets};
use converter_sniffer::Format;
use serde_json::{json, Value};

use crate::budget::{budget, MemoryProbe};
use crate::queue::{phase1_pools, Admission, Scheduler, Ticket, STARVATION_AFTER};
use crate::verify::verify_outputs;
use crate::worker::{
    failure, CancelToken, LaunchSpec, Launcher, RequestKind, Timing, Watchdog, Worker, WorkerEvent,
};

/// Code shown for a worker warning that isn't in the catalog. The UI shows
/// generic text for it; the original code is logged.
pub const GENERIC_WARNING_CODE: &str = "W_UNKNOWN";

/// Longest worker `detail` kept, in bytes. Shown as plain text only.
pub const MAX_DETAIL_BYTES: usize = 1000;

/// The scheduler pool for an adapter: `redact` for `pdf-redact`, otherwise
/// its manifest's `workerClass`.
pub fn pool_for(adapter_id: &str, worker_class: &str) -> String {
    if adapter_id == "pdf-redact" {
        "redact".to_owned()
    } else {
        worker_class.to_owned()
    }
}

/// Everything needed to run one staged job.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// A job whose inputs are staged; it must be in `staging`.
    pub job: JobId,
    pub pool: String,
    /// What runs, in order. Most jobs have one part; converting a mixed
    /// selection ("all to PDF" for Word files and photos) has one per
    /// adapter (ADR 0003).
    pub parts: Vec<JobPart>,
}

/// One adapter's share of a job.
#[derive(Debug, Clone)]
pub struct JobPart {
    pub adapter_id: String,
    /// Indexes of the staged inputs (`in/0.pdf` is 0) this part gets, in order.
    pub inputs: Vec<usize>,
    pub timeout_base_s: u32,
    pub timeout_s_per_mb: f64,
    pub no_progress_s: u32,
    /// Already validated against the adapter's params schema.
    pub params: Value,
    pub secrets: Option<Secrets>,
    /// The format every output of this part must sniff as. Parts can
    /// differ: a tool that keeps each file's format runs one part per
    /// format (ADR 0004).
    pub output_format: Format,
}

impl JobSpec {
    /// A job run by one adapter over every staged input.
    #[allow(clippy::too_many_arguments)]
    pub fn single(
        job: JobId,
        adapter_id: &str,
        pool: &str,
        output_format: Format,
        inputs: usize,
        (timeout_base_s, timeout_s_per_mb, no_progress_s): (u32, f64, u32),
        params: Value,
        secrets: Option<Secrets>,
    ) -> Self {
        JobSpec {
            job,
            pool: pool.to_owned(),
            parts: vec![JobPart {
                adapter_id: adapter_id.to_owned(),
                inputs: (0..inputs).collect(),
                timeout_base_s,
                timeout_s_per_mb,
                no_progress_s,
                params,
                secrets,
                output_format,
            }],
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub pools: HashMap<String, usize>,
    /// The user's memory budget setting.
    pub user_memory_limit: Option<u64>,
    pub timing: Timing,
    pub starvation: Duration,
    /// Progress events closer together than this are dropped (10 Hz).
    pub progress_interval: Duration,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        RunnerConfig {
            pools: phase1_pools(),
            user_memory_limit: None,
            timing: Timing::default(),
            starvation: STARVATION_AFTER,
            progress_interval: Duration::from_millis(100),
        }
    }
}

struct Shared {
    scheduler: Scheduler,
    admitted: HashMap<JobId, Admission>,
}

/// Runs jobs on their own threads, sharing one admission queue.
pub struct Runner {
    store: Arc<JobStore>,
    launcher: Arc<dyn Launcher>,
    memory: Arc<dyn MemoryProbe>,
    codec: Arc<Codec>,
    config: RunnerConfig,
    shared: Mutex<Shared>,
    wake: Condvar,
    tokens: Mutex<HashMap<JobId, CancelToken>>,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner").finish_non_exhaustive()
    }
}

impl Runner {
    pub fn new(
        store: Arc<JobStore>,
        launcher: Arc<dyn Launcher>,
        memory: Arc<dyn MemoryProbe>,
        config: RunnerConfig,
    ) -> Arc<Self> {
        let scheduler = Scheduler::with_starvation(config.pools.clone(), config.starvation);
        Arc::new(Runner {
            store,
            launcher,
            memory,
            codec: Arc::new(Codec::new()),
            config,
            shared: Mutex::new(Shared {
                scheduler,
                admitted: HashMap::new(),
            }),
            wake: Condvar::new(),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Starts running a staged job on its own thread. The job ends in
    /// `succeeded`, `failed` or `cancelled`.
    pub fn start(self: &Arc<Self>, spec: JobSpec) -> JoinHandle<()> {
        let token = CancelToken::new();
        lock(&self.tokens).insert(spec.job.clone(), token.clone());
        let me = self.clone();
        thread::spawn(move || {
            let job = spec.job.clone();
            match me.drive(&spec, &token) {
                Ok(()) => {}
                Err(f) if f.code == "E_CANCELLED" => {
                    let _ = me.store.transition(&job, JobState::Cancelled);
                }
                Err(f) => {
                    let _ = me.store.fail(&job, f);
                }
            }
            lock(&me.tokens).remove(&job);
        })
    }

    /// Asks a job to stop. The worker gets `cancel`, then is killed after
    /// the grace period. False if the job isn't running here.
    pub fn cancel(&self, job: &JobId) -> bool {
        let found = match lock(&self.tokens).get(job) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        };
        self.wake.notify_all();
        found
    }

    fn drive(&self, spec: &JobSpec, token: &CancelToken) -> Result<(), JobFailure> {
        let job = &spec.job;
        if spec.parts.is_empty() {
            return Err(failure("E_INTERNAL"));
        }
        let dirs = self
            .store
            .job_dirs(job)
            .map_err(|_| failure("E_INTERNAL"))?;
        let staged: Vec<(String, u64)> = dirs
            .inputs
            .iter()
            .filter_map(|p| {
                let name = p.file_name()?.to_string_lossy();
                let size = std::fs::metadata(p).map_or(0, |m| m.len());
                Some((format!("in/{name}"), size))
            })
            .collect();
        let mut plans = Vec::with_capacity(spec.parts.len());
        for part in &spec.parts {
            let mut inputs = Vec::with_capacity(part.inputs.len());
            let mut bytes = 0u64;
            for &i in &part.inputs {
                let (path, size) = staged.get(i).ok_or_else(|| failure("E_INTERNAL"))?;
                inputs.push(path.clone());
                bytes += size;
            }
            if inputs.is_empty() {
                return Err(failure("E_INTERNAL"));
            }
            let watchdog = Watchdog::from_limits(
                part.timeout_base_s,
                part.timeout_s_per_mb,
                part.no_progress_s,
                bytes,
            );
            plans.push((inputs, watchdog));
        }
        let launch = |part: &JobPart, memory_limit| LaunchSpec {
            adapter_id: part.adapter_id.clone(),
            job_id: job.clone(),
            job_dir: dirs.root.clone(),
            memory_limit,
        };
        let mut events = EventForwarder::new(&self.store, job, self.config.progress_interval);

        // Every part is checked before any runs, so a problem with the last
        // file shows before minutes of work on the first.
        check(token)?;
        self.set_state(job, JobState::Validating)?;
        let mut estimate = 0u64;
        for (part, (inputs, watchdog)) in spec.parts.iter().zip(&plans) {
            let mut worker = Worker::spawn(
                &*self.launcher,
                &launch(part, None),
                self.codec.clone(),
                self.config.timing,
            )?;
            let validated = worker.request(
                RequestKind::Validate,
                json!({ "inputs": inputs, "params": part.params }),
                part.secrets.as_ref(),
                *watchdog,
                token,
                &mut |e| events.forward(e),
            )?;
            for w in validated["warnings"].as_array().into_iter().flatten() {
                events.warning(w["code"].as_str().unwrap_or_default(), w.get("detail"));
            }
            if let Some(first) = validated["errors"].as_array().and_then(|e| e.first()) {
                return Err(failure(first["code"].as_str().unwrap_or("E_INTERNAL")));
            }
            let estimated = worker.request(
                RequestKind::Estimate,
                json!({ "inputs": inputs, "params": part.params, "metadata": validated["metadata"] }),
                None,
                *watchdog,
                token,
                &mut |e| events.forward(e),
            )?;
            // Parts run one at a time, so the largest one sets the budget.
            estimate = estimate.max(estimated["ramBytes"].as_u64().unwrap_or(0));
            // Don't hold an idle worker while queued; execute gets a fresh one
            // launched with the admitted memory limit.
            worker.shutdown();
        }

        let mut claimed: Vec<(Value, Format)> = Vec::new();
        let mut next = 0;
        let mut retry = false;
        let mut warned_large = false;
        while next < spec.parts.len() {
            self.set_state(job, JobState::Queued)?;
            let admission = self.admit(job, &spec.pool, estimate, retry, token)?;
            let slot = SlotGuard { runner: self, job };
            self.set_state(job, JobState::Running)?;
            if admission.large && !warned_large {
                warned_large = true;
                events.warning("W_LARGE_JOB", None);
            }
            while next < spec.parts.len() {
                let part = &spec.parts[next];
                let (inputs, watchdog) = &plans[next];
                let mut worker = Worker::spawn(
                    &*self.launcher,
                    &launch(part, Some(admission.hard_limit)),
                    self.codec.clone(),
                    self.config.timing,
                )?;
                let outcome = worker.request(
                    RequestKind::Execute,
                    json!({ "inputs": inputs, "outDir": "out", "workDir": "work", "params": part.params }),
                    part.secrets.as_ref(),
                    *watchdog,
                    token,
                    &mut |e| events.forward(e),
                );
                match outcome {
                    Ok(result) => {
                        worker.shutdown();
                        let taken = take_outputs(
                            &dirs.out,
                            &result,
                            (spec.parts.len() > 1).then_some(next),
                        )?;
                        claimed.extend(taken.into_iter().map(|o| (o, part.output_format)));
                        next += 1;
                    }
                    // Spec 10.2: retry once, alone, at the full budget.
                    Err(f) if f.code == "E_RESOURCE_MEMORY" && !retry => {
                        drop(worker);
                        retry = true;
                        break;
                    }
                    Err(f) => return Err(f),
                }
            }
            // The budget is released only once the worker is gone.
            drop(slot);
        }

        // Checked once every worker has exited, so none can change a file
        // after its check.
        check(token)?;
        self.set_state(job, JobState::Verifying)?;
        verify_outputs(&self.store, job, &claimed)?;
        self.set_state(job, JobState::Succeeded)?;
        Ok(())
    }

    fn set_state(&self, job: &JobId, state: JobState) -> Result<(), JobFailure> {
        self.store
            .transition(job, state)
            .map(|_| ())
            .map_err(|_| failure("E_INTERNAL"))
    }

    /// Queues the job and blocks until it's admitted or cancelled. The memory
    /// budget is re-read before every admission round.
    fn admit(
        &self,
        job: &JobId,
        pool: &str,
        estimate: u64,
        retry_alone: bool,
        token: &CancelToken,
    ) -> Result<Admission, JobFailure> {
        let mut shared = lock(&self.shared);
        shared.scheduler.enqueue(Ticket {
            job: job.clone(),
            pool: pool.to_owned(),
            estimate,
            retry_alone,
            enqueued: Instant::now(),
        });
        loop {
            if token.is_cancelled() {
                shared.scheduler.dequeue(job);
                if shared.admitted.remove(job).is_some() {
                    shared.scheduler.finish(job);
                }
                self.wake.notify_all();
                return Err(failure("E_CANCELLED"));
            }
            if let Some(a) = shared.admitted.remove(job) {
                return Ok(a);
            }
            // Running jobs' memory is already used in the OS's view; add
            // their reservations back so they aren't counted twice.
            let available = self
                .memory
                .available_bytes()
                .saturating_add(shared.scheduler.reserved());
            let budget = budget(available, self.config.user_memory_limit);
            let admitted = shared.scheduler.admit(Instant::now(), budget);
            if !admitted.is_empty() {
                for a in admitted {
                    shared.admitted.insert(a.job.clone(), a);
                }
                self.wake.notify_all();
                continue;
            }
            shared = self
                .wake
                .wait_timeout(shared, Duration::from_millis(250))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// Releases a running job's slot when dropped, and wakes queued jobs.
struct SlotGuard<'a> {
    runner: &'a Runner,
    job: &'a JobId,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        lock(&self.runner.shared).scheduler.finish(self.job);
        self.runner.wake.notify_all();
    }
}

fn check(token: &CancelToken) -> Result<(), JobFailure> {
    if token.is_cancelled() {
        Err(failure("E_CANCELLED"))
    } else {
        Ok(())
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Turns worker events into job store events: stages and warnings at once,
/// progress at most every `interval`.
struct EventForwarder<'a> {
    store: &'a JobStore,
    job: &'a JobId,
    interval: Duration,
    last_progress: Option<Instant>,
}

impl<'a> EventForwarder<'a> {
    fn new(store: &'a JobStore, job: &'a JobId, interval: Duration) -> Self {
        EventForwarder {
            store,
            job,
            interval,
            last_progress: None,
        }
    }

    fn forward(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Stage { name, .. } => {
                let _ = self.store.set_stage(self.job, &name);
                self.last_progress = None;
            }
            WorkerEvent::Progress { done, total, .. } => {
                let now = Instant::now();
                if self
                    .last_progress
                    .is_some_and(|t| now.duration_since(t) < self.interval)
                {
                    return;
                }
                self.last_progress = Some(now);
                let _ = self.store.set_progress(self.job, Progress { done, total });
            }
            WorkerEvent::Warning { code, detail } => self.warning(&code, detail.as_ref()),
        }
    }

    fn warning(&mut self, code: &str, detail: Option<&Value>) {
        let code = if warning_catalog().contains(code) {
            code.to_owned()
        } else {
            // Codes match ^[A-Z][A-Z0-9_]{0,63}$, so this is safe to log.
            log::warn!("worker sent a warning code outside the catalog: {code}");
            GENERIC_WARNING_CODE.to_owned()
        };
        let detail = detail.map(|d| truncate(d.to_string(), MAX_DETAIL_BYTES));
        let _ = self.store.add_warning(self.job, Warning { code, detail });
    }
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Warning codes in the catalog, read from the protocol schema.
pub fn warning_catalog() -> &'static HashSet<String> {
    static CATALOG: OnceLock<HashSet<String>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(converter_protocol::SCHEMA_JSON).expect("embedded schema is JSON");
        schema["definitions"]["WarningCode"]["enum"]
            .as_array()
            .expect("the schema lists warning codes")
            .iter()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect()
    })
}

/// The outputs a part's worker claims. With several parts, each part's
/// files are renamed `p<part>-<name>` before the next part runs, since
/// every worker numbers its outputs from 0. A claim the rename can't follow
/// (a missing file, a path outside `out/`) is a protocol error.
fn take_outputs(out: &Path, result: &Value, part: Option<usize>) -> Result<Vec<Value>, JobFailure> {
    let claimed = result["outputs"].as_array().cloned().unwrap_or_default();
    if claimed.is_empty() {
        return Err(failure("E_PROTOCOL"));
    }
    let Some(part) = part else {
        return Ok(claimed);
    };
    claimed
        .into_iter()
        .map(|mut o| {
            let name = o["path"]
                .as_str()
                .and_then(|p| p.strip_prefix("out/"))
                .filter(|n| !n.is_empty() && !n.contains(['/', '\\']) && *n != "..")
                .ok_or_else(|| failure("E_PROTOCOL"))?
                .to_owned();
            let renamed = format!("p{part}-{name}");
            std::fs::rename(out.join(&name), out.join(&renamed))
                .map_err(|_| failure("E_PROTOCOL"))?;
            o["path"] = json!(format!("out/{renamed}"));
            Ok(o)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_comes_from_the_schema() {
        assert!(warning_catalog().contains("TAGS_DROPPED"));
        assert!(warning_catalog().contains("W_LARGE_JOB"));
        assert!(!warning_catalog().contains(GENERIC_WARNING_CODE));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("é".repeat(10), 5), "éé");
        assert_eq!(truncate("abc".into(), 10), "abc");
    }

    #[test]
    fn redact_gets_its_own_pool() {
        assert_eq!(pool_for("pdf-redact", "pdf"), "redact");
        assert_eq!(pool_for("pdf-merge", "pdf"), "pdf");
    }
}
