//! Jobs: targets, creating and starting jobs, saving and discarding.

use std::path::PathBuf;

use converter_jobstore::{
    FolderLayout, JobId, JobSnapshot, SaveCheck, SaveDestination, SaveOptions, SaveReport, Snapshot,
};
use converter_router::manifest::Kind;
use converter_router::{
    targets, AdapterEntry, Format as RouterFormat, OutputFormat, Planner, Registry, Targets,
};
use converter_scheduler::{pool_for, JobPart, JobSpec};
use converter_sniffer::Format;
use serde_json::{json, Value};

use crate::job_parts::{
    best_plan, collect_inputs, finish_params, flatten, formats_in_order, group_by_adapter,
    group_by_format, order_files, skip_same_format, split_secrets, strictest_pool, worker_class,
    Collected, Flattened, Prepared,
};
use crate::{lock, parse_format, App, CommandError, CommandResult, Reason};

impl App {
    // ------------------------------------------------------------- targets

    /// Operations and conversions for these files (spec 11.1 `get_targets`).
    pub fn get_targets(&self, file_ids: &[String]) -> CommandResult<Targets> {
        let formats = self.formats_of(file_ids)?;
        let mut t = targets(&self.registry, &self.availability, &formats);
        // Every offered conversion runs as one job in one adapter (ADR 0003).
        for g in &mut t.groups {
            g.conversions
                .retain(|c| c.plan.as_ref().is_none_or(|p| p.hops.len() == 1));
        }
        // A shared target ("all to PDF") needs that one-job route in every
        // group. The groups may use different adapters: each becomes a part
        // of the job.
        let groups = t.groups.clone();
        t.shared.retain(|s| {
            groups.iter().all(|g| {
                g.conversions
                    .iter()
                    .any(|c| c.to == s.to && c.plan.is_some() && c.unavailable.is_none())
            })
        });
        Ok(t)
    }

    /// Every adapter's manifest and parameter schema, for generated forms.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    // ---------------------------------------------------------------- jobs

    /// Validates params, stages inputs and starts an operation (spec 11.1).
    pub fn create_job(
        &self,
        operation_id: &str,
        file_ids: &[String],
        params: Value,
    ) -> CommandResult<JobId> {
        self.require_sandbox()?;
        let entry = self
            .registry
            .get(operation_id)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownOperation))?;
        // `output-check` and any other `kind: check` run for the core only
        // (ADR 0006), though their manifests look like an operation's.
        let (inputs, outputs) = match (&entry.manifest.inputs, &entry.manifest.outputs) {
            (Some(i), Some(o)) if entry.manifest.kind == Kind::Operation => (i, o),
            _ => return Err(CommandError::with("E_INTERNAL", Reason::NotAnOperation)),
        };
        if self.availability_of(entry).is_err() {
            return Err(CommandError::with(
                "E_INSTALL_CORRUPT",
                Reason::EngineMissing,
            ));
        }
        let fixed = match outputs.format {
            OutputFormat::Fixed(f) => Some(f),
            OutputFormat::Chosen => Some(
                params["to"]
                    .as_str()
                    .and_then(RouterFormat::parse)
                    .ok_or_else(|| CommandError::with("E_PARAMS_INVALID", Reason::UnknownFormat))?,
            ),
            OutputFormat::Same => None,
        };
        // The `order` param names every selected file, so it is applied to the
        // whole selection, before any grouping into parts.
        let file_ids = order_files(&params, file_ids)?;
        if let Some(format) = fixed {
            let output_format = parse_format(format.as_str()).ok_or_else(|| {
                CommandError::with("E_INSTALL_CORRUPT", Reason::UnknownOutputFormat)
            })?;
            let mut part = self.prepare(
                entry,
                &file_ids,
                params,
                &|f| inputs.formats.contains(&f),
                Some((inputs.min, inputs.max)),
            )?;
            part.part.output_format = output_format;
            return self.start(vec![part]);
        }

        // Each file keeps its format (ADR 0004): one part per format, in the
        // order the formats first appear.
        let count = u32::try_from(file_ids.len()).unwrap_or(u32::MAX);
        if count < inputs.min || count > inputs.max {
            return Err(CommandError::with("E_INTERNAL", Reason::WrongFileCount));
        }
        let formats = self.formats_of(&file_ids)?;
        let groups = group_by_format(&file_ids, &formats);
        let mut parts = Vec::with_capacity(groups.len());
        for (format, ids) in &groups {
            let mut part = self.prepare(
                entry,
                ids,
                params.clone(),
                &|f| f == *format && inputs.formats.contains(&f),
                None,
            )?;
            part.part.output_format = parse_format(format.as_str())
                .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownFormat))?;
            parts.push(part);
        }
        self.start(parts)
    }

    /// Converts every file to `to`, in one job (ADR 0003). Files that need
    /// different adapters (Word files and photos to PDF) run as one part per
    /// adapter; each part gets the params its schema knows, so a mixed
    /// selection runs with every adapter's defaults.
    ///
    /// With `"skipSame": true` in `params`, files already in the target
    /// format are left out; see [`App::create_conversion_counted`].
    pub fn create_conversion(
        &self,
        file_ids: &[String],
        to: &str,
        params: Value,
    ) -> CommandResult<JobId> {
        self.create_conversion_counted(file_ids, to, params)
            .map(|(job, _)| job)
    }

    /// [`App::create_conversion`], also returning how many files `skipSame`
    /// left out. If every file is skipped there is nothing to run and the
    /// error is `E_PARAMS_INVALID` / `allSkipped`.
    pub fn create_conversion_counted(
        &self,
        file_ids: &[String],
        to: &str,
        mut params: Value,
    ) -> CommandResult<(JobId, usize)> {
        self.require_sandbox()?;
        let target = RouterFormat::parse(to)
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownFormat))?;
        let skip_same = params
            .as_object_mut()
            .and_then(|m| m.remove("skipSame"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let formats = self.formats_of(file_ids)?;
        let (file_ids, formats, skipped) = if skip_same {
            let kept = skip_same_format(file_ids, &formats, target);
            if kept.0.is_empty() {
                return Err(CommandError::with("E_PARAMS_INVALID", Reason::AllSkipped));
            }
            // An `order` naming a skipped file would never match.
            if let Some(order) = params.get_mut("order").and_then(Value::as_array_mut) {
                order.retain(|v| v.as_str().is_some_and(|id| kept.0.iter().any(|k| k == id)));
            }
            kept
        } else {
            (file_ids.to_vec(), formats, 0)
        };
        // Ordered before grouping into parts, like `create_job`.
        let ordered = order_files(&params, &file_ids)?;
        let formats = formats_in_order(&file_ids, &formats, &ordered);
        let file_ids = ordered;
        let planner = Planner::new(&self.registry, &self.availability);
        let groups = group_by_adapter(&file_ids, &formats, |from| {
            let plan = best_plan(&planner, from, target)
                .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::NoRoute))?;
            if plan.hops.len() != 1 {
                return Err(CommandError::with("E_INTERNAL", Reason::MultiHop));
            }
            Ok(plan.hops[0].adapter.clone())
        })?;
        if groups.is_empty() {
            return Err(CommandError::with("E_INTERNAL", Reason::NoFiles));
        }
        let Value::Object(given) = params else {
            return Err(CommandError::with("E_PARAMS_INVALID", Reason::NotAnObject));
        };
        let output_format = parse_format(target.as_str())
            .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::UnknownFormat))?;
        let mixed = groups.len() > 1;
        let mut parts = Vec::with_capacity(groups.len());
        for (adapter, ids, sources) in &groups {
            let entry = self
                .registry
                .get(adapter)
                .ok_or_else(|| CommandError::with("E_INSTALL_CORRUPT", Reason::UnknownAdapter))?;
            let known = &entry.params_schema["properties"];
            let mut map: serde_json::Map<String, Value> = given
                .iter()
                .filter(|(k, _)| !mixed || known.get(k.as_str()).is_some())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            map.insert("to".into(), json!(target.as_str()));
            let mut part = self.prepare(
                entry,
                ids,
                Value::Object(map),
                &|f| sources.contains(&f),
                None,
            )?;
            part.part.output_format = output_format;
            parts.push(part);
        }
        Ok((self.start(parts)?, skipped))
    }

    pub(crate) fn availability_of(&self, entry: &AdapterEntry) -> Result<(), ()> {
        let installed = self.availability.engines.as_ref();
        let ok = entry
            .manifest
            .platforms
            .contains(&self.availability.platform)
            && entry
                .manifest
                .requires
                .iter()
                .all(|e| installed.is_none_or(|i| i.contains(e)));
        if ok {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Validates params and gathers what a job part needs. `file_ids` are
    /// already in input order (see [`order_files`]).
    ///
    /// One password serves all of a part's files: the worker protocol has a
    /// single `password` secret per job part. Files with different passwords
    /// are refused up front (`passwordsDiffer`) instead of failing in the
    /// worker as if the password were wrong.
    pub(crate) fn prepare(
        &self,
        entry: &AdapterEntry,
        file_ids: &[String],
        params: Value,
        accepts: &dyn Fn(RouterFormat) -> bool,
        count_limits: Option<(u32, u32)>,
    ) -> CommandResult<Prepared> {
        let manifest = &entry.manifest;
        let (mut params, secrets) = split_secrets(params)?;
        let validator = jsonschema::validator_for(&entry.params_schema)
            .map_err(|_| CommandError::with("E_INSTALL_CORRUPT", Reason::BadParamsSchema))?;
        let issues: Vec<Value> = validator
            .iter_errors(&params)
            .take(32)
            .map(|e| json!({ "path": e.instance_path().to_string() }))
            .collect();
        if !issues.is_empty() {
            return Err(CommandError {
                code: "E_PARAMS_INVALID".into(),
                reason: None,
                issues,
            });
        }

        let count = u32::try_from(file_ids.len()).unwrap_or(u32::MAX);
        if let Some((min, max)) = count_limits {
            if count < min || count > max {
                return Err(CommandError::with("E_INTERNAL", Reason::WrongFileCount));
            }
        }
        if count == 0 {
            return Err(CommandError::with("E_INTERNAL", Reason::WrongFileCount));
        }

        let Collected {
            sources,
            folders,
            password,
            titles,
        } = collect_inputs(&lock(&self.files), file_ids, accepts)?;

        let wants_titles = entry.params_schema["properties"]
            .get("fileTitles")
            .is_some();
        finish_params(&mut params, wants_titles, &titles);
        let mut secrets = secrets.unwrap_or_default();
        if secrets.password.is_none() {
            secrets.password = password;
        }
        // For a tool that declares one (ADR 0006), from the user's own params.
        let recheck = self.recheck_for(entry, file_ids, &params)?;
        let has_secrets = secrets.password.is_some()
            || secrets.user_password.is_some()
            || secrets.owner_password.is_some();
        Ok(Prepared {
            file_ids: file_ids.to_vec(),
            pool: pool_for(&manifest.id, worker_class(manifest)),
            disk_factor: manifest.limits.disk_factor,
            part: JobPart {
                adapter_id: manifest.id.clone(),
                inputs: Vec::new(),
                timeout_base_s: manifest.limits.timeout_base_s,
                timeout_s_per_mb: manifest.limits.timeout_s_per_mb,
                no_progress_s: manifest.limits.no_progress_s,
                params,
                secrets: has_secrets.then_some(secrets),
                // Set by the caller, which knows the target.
                output_format: Format::Pdf,
                recheck,
            },
            sources,
            folders,
        })
    }

    /// Stages every part's inputs in one job and hands it to the scheduler.
    pub(crate) fn start(&self, parts: Vec<Prepared>) -> CommandResult<JobId> {
        let disk_factor = parts.iter().map(|p| p.disk_factor).fold(0.0, f64::max);
        let pool = strictest_pool(parts.iter().map(|p| p.pool.as_str()));
        let Flattened {
            sources,
            folders,
            file_ids,
            parts: job_parts,
        } = flatten(parts);

        let (job, _) = self.store.create_job().map_err(CommandError::from_job)?;
        if let Err(e) = self.store.stage_inputs(&job, &sources, disk_factor) {
            let failure = converter_jobstore::JobFailure {
                code: e.code().unwrap_or("E_INTERNAL").into(),
                retryable: false,
            };
            // Failed and then deleted: a job that never started must not
            // stay in the store (and in the snapshot) for the session.
            let _ = self.store.fail(&job, failure);
            let _ = self.store.discard(&job);
            return Err(CommandError::from_job(e));
        }
        if folders.iter().any(Option::is_some) {
            lock(&self.layouts).insert(job.clone(), folders);
        }
        lock(&self.inputs).insert(job.clone(), file_ids);
        let handle = self.runner.start(JobSpec {
            job: job.clone(),
            pool,
            parts: job_parts,
        });
        let mut runners = lock(&self.runners);
        // Finished threads are joined here, so the list stays as long as the
        // number of jobs running, not the number ever started.
        let (done, running): (Vec<_>, Vec<_>) = std::mem::take(&mut *runners)
            .into_iter()
            .partition(|h| h.is_finished());
        for h in done {
            let _ = h.join();
        }
        *runners = running;
        runners.push(handle);
        Ok(job)
    }

    /// The file IDs of a job's staged inputs, in staged order, so a warning's
    /// `detail.inputs` indices can be turned back into files. Known until the
    /// job is saved or discarded.
    pub fn job_input_files(&self, job: &JobId) -> CommandResult<Vec<String>> {
        lock(&self.inputs)
            .get(job)
            .cloned()
            .ok_or_else(|| CommandError::with("E_UNKNOWN_JOB", Reason::UnknownJob))
    }

    pub fn cancel_job(&self, job: &JobId) -> CommandResult<bool> {
        Ok(self.runner.cancel(job))
    }

    pub fn discard_job(&self, job: &JobId) -> CommandResult<()> {
        // The worker can't run in a directory about to be deleted, but
        // previews and layout stay valid if the discard is refused.
        self.stop_output_worker(job);
        match self.store.discard(job) {
            Ok(()) => {
                self.forget_output_previews(job);
                lock(&self.layouts).remove(job);
                lock(&self.inputs).remove(job);
                Ok(())
            }
            Err(e) => {
                if matches!(e, converter_jobstore::JobError::UnknownJob(_)) {
                    self.forget_output_previews(job);
                    lock(&self.layouts).remove(job);
                    lock(&self.inputs).remove(job);
                }
                Err(CommandError::from_job(e))
            }
        }
    }

    pub fn check_save(&self, job: &JobId, dest: &SaveDestination) -> CommandResult<SaveCheck> {
        self.store
            .check_save(job, dest)
            .map_err(CommandError::from_job)
    }

    /// Saves outputs to a destination from the native dialog, then deletes
    /// the job. The report says where each output went. If the save is
    /// refused (a confirmation is needed, the destination is full), the job
    /// keeps its previews and folder layout so the user can try again.
    pub fn save_outputs(
        &self,
        job: &JobId,
        dest: &SaveDestination,
        confirmed_input_overwrite: bool,
    ) -> CommandResult<SaveReport> {
        // The preview worker's process stops first (it can't run in a job
        // directory that is being deleted); its preview tokens are kept
        // until the save has succeeded.
        self.stop_output_worker(job);
        let report = self
            .store
            .save_outputs(
                job,
                dest,
                SaveOptions {
                    confirmed_input_overwrite,
                },
            )
            .map_err(CommandError::from_job)?;
        self.forget_output_previews(job);
        lock(&self.layouts).remove(job);
        lock(&self.inputs).remove(job);
        Ok(report)
    }

    /// Whether saving this job's outputs can keep the dropped folder's
    /// layout: at least one output came from a file that was in a folder.
    pub fn can_keep_layout(&self, job: &JobId) -> bool {
        let Ok(snap) = self.job_snapshot(job) else {
            return false;
        };
        let layouts = lock(&self.layouts);
        let Some(inputs) = layouts.get(job) else {
            return false;
        };
        snap.outputs.iter().any(|o| {
            o.source
                .and_then(|s| inputs.get(s))
                .is_some_and(Option::is_some)
        })
    }

    /// The destination for saving several outputs into `dir`: with the
    /// dropped folder's layout when asked for and possible.
    pub fn folder_destination(
        &self,
        job: &JobId,
        dir: PathBuf,
        keep_layout: bool,
    ) -> SaveDestination {
        if keep_layout && self.can_keep_layout(job) {
            if let Some(inputs) = lock(&self.layouts).get(job).cloned() {
                return SaveDestination::FolderKeepingLayout(FolderLayout { dir, inputs });
            }
        }
        SaveDestination::Folder(dir)
    }

    pub(crate) fn job_snapshot(&self, job: &JobId) -> CommandResult<JobSnapshot> {
        self.store
            .job_snapshot(job)
            .map_err(|_| CommandError::with("E_UNKNOWN_JOB", Reason::UnknownJob))
    }

    /// Current state and latest `seq`, for reload recovery (spec 11.2).
    pub fn get_snapshot(&self) -> Snapshot {
        let hidden = self.events.hidden_jobs();
        let mut snap = self.store.snapshot();
        snap.jobs.retain(|j| !hidden.contains(&j.job_id));
        snap
    }
}
