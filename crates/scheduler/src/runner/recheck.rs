//! The re-check of a job's outputs in a separate check job (ADR 0006).

use std::path::{Path, PathBuf};

use converter_jobstore::{JobFailure, JobId};
use serde_json::{json, Value};

use super::{check, Recheck, Runner, SlotGuard};
use crate::verify::copy_verified;
use crate::worker::{failure, CancelToken, LaunchSpec, RequestKind, Watchdog, Worker};

/// A report is a few check IDs and counts; anything bigger isn't one.
const MAX_REPORT_BYTES: u64 = 64 * 1024;

/// The check job's folder inside the tool job's own folder, so it goes when
/// that job is discarded, and is removed here in every case.
struct CheckDir(PathBuf);

impl CheckDir {
    fn create(root: &Path, id: &JobId) -> std::io::Result<CheckDir> {
        let path = root.join(format!("recheck-{}", id.as_str()));
        let dir = CheckDir(path);
        for sub in ["", "in", "work", "out"] {
            private_dir(&dir.0.join(sub))?;
        }
        Ok(dir)
    }
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(path)
    }
}

impl Drop for CheckDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Runner {
    /// Runs `rc` over copies of `files` (path, format, SHA-256 recorded at
    /// verification) and returns `Ok` only
    /// if its report says every check passed.
    ///
    /// The check job has its own folder and its own worker, and sees nothing
    /// of the tool job but these copies. A check that fails, can't start,
    /// times out or reports something unreadable fails the tool's job with
    /// `rc.failure_code`; that a check could not run is never a pass. A
    /// cancelled tool job stops the check (`E_CANCELLED`).
    pub(super) fn recheck(
        &self,
        root: &Path,
        pool: &str,
        estimate: u64,
        rc: &Recheck,
        files: &[(PathBuf, &str, &str)],
        token: &CancelToken,
    ) -> Result<(), JobFailure> {
        check(token)?;
        let fail = || failure(&rc.failure_code);
        if files.is_empty() {
            return Err(fail());
        }
        let id = JobId::new_random();
        let dir = CheckDir::create(root, &id).map_err(|_| fail())?;
        let mut inputs = Vec::with_capacity(files.len());
        let mut bytes = 0u64;
        for (n, (source, ext, sha256)) in files.iter().enumerate() {
            // The copy is hashed as it is made: a link, or a file that changed
            // since verification, is refused rather than checked.
            let name = format!("in/{n}.{ext}");
            if copy_verified(source, &dir.0.join(&name), sha256).is_err() {
                return Err(fail());
            }
            bytes += std::fs::metadata(dir.0.join(&name)).map_or(0, |m| m.len());
            inputs.push(name);
        }
        let watchdog = Watchdog::from_limits(
            rc.timeout_base_s,
            rc.timeout_s_per_mb,
            rc.no_progress_s,
            bytes,
        );

        // The check queues in the tool's pool, behind the same budget.
        let admission = self.admit(&id, pool, estimate, false, token)?;
        let _slot = SlotGuard {
            runner: self,
            job: &id,
        };
        let outcome = (|| {
            let mut worker = Worker::spawn(
                &*self.launcher,
                &LaunchSpec {
                    adapter_id: rc.adapter_id.clone(),
                    job_id: id.clone(),
                    job_dir: dir.0.clone(),
                    memory_limit: Some(admission.hard_limit),
                },
                self.codec.clone(),
                self.config.timing,
            )?;
            let result = worker.request(
                RequestKind::Execute,
                json!({ "inputs": inputs, "outDir": "out", "workDir": "work", "params": rc.params }),
                None,
                watchdog,
                token,
                &mut |_| {},
            )?;
            worker.shutdown();
            report_passed(&dir.0, &result)
        })();
        match outcome {
            Ok(()) => Ok(()),
            Err(f) if f.code == "E_CANCELLED" => Err(f),
            Err(f) => {
                // Codes only: nothing from the documents.
                log::warn!("an output re-check failed: {}", f.code);
                Err(fail())
            }
        }
    }
}

/// Reads the report the check worker claims. `Ok` only for
/// `{"passed": true}`; the failed check IDs are logged.
fn report_passed(dir: &Path, result: &Value) -> Result<(), JobFailure> {
    let unreadable = || failure("E_PROTOCOL");
    let name = result["outputs"][0]["path"]
        .as_str()
        .and_then(|p| p.strip_prefix("out/"))
        .filter(|n| !n.is_empty() && !n.contains(['/', '\\']) && *n != "..")
        .ok_or_else(unreadable)?;
    let path = dir.join("out").join(name);
    let meta = std::fs::symlink_metadata(&path).map_err(|_| unreadable())?;
    if !meta.is_file() || meta.len() > MAX_REPORT_BYTES {
        return Err(unreadable());
    }
    let report: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|_| unreadable())?)
        .map_err(|_| unreadable())?;
    if report["passed"] == json!(true) {
        return Ok(());
    }
    let failed: Vec<&str> = report["failed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .take(16)
        .collect();
    log::warn!("an output re-check failed checks: {failed:?}");
    Err(failure("E_OUTPUT_INVALID"))
}
