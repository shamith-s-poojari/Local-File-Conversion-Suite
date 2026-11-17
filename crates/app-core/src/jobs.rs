//! Jobs: targets, creating and starting jobs, saving and discarding.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use converter_jobstore::{
    FolderLayout, InputSource, JobId, JobSnapshot, SaveCheck, SaveDestination, SaveOptions,
    SaveReport, Snapshot,
};
use converter_protocol::{SecretString, Secrets};
use converter_router::manifest::Kind;
use converter_router::{
    matrix, targets, AdapterEntry, Format as RouterFormat, OutputFormat, Plan, Planner, Registry,
    Targets,
};
use converter_scheduler::{pool_for, JobPart, JobSpec};
use converter_sniffer::Format;
use serde_json::{json, Value};

use zeroize::Zeroizing;

use crate::files::FileEntry;
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
            let _ = self.store.fail(&job, failure);
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
        let hidden = lock(&self.hidden).clone();
        let mut snap = self.store.snapshot();
        snap.jobs.retain(|j| !hidden.contains(&j.job_id));
        snap
    }
}

/// `pdf-protect` collects passwords in password widgets; they must travel
/// as secrets, never params (spec 7.3).
fn split_secrets(params: Value) -> CommandResult<(Value, Option<Secrets>)> {
    let Value::Object(mut map) = params else {
        return Err(CommandError::with("E_PARAMS_INVALID", Reason::NotAnObject));
    };
    let take = |map: &mut serde_json::Map<String, Value>,
                key: &str|
     -> CommandResult<Option<SecretString>> {
        match map.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.is_empty() => Ok(Some(SecretString::new(s))),
            Some(Value::String(_)) => Ok(None),
            Some(_) => Err(CommandError::with(
                "E_PARAMS_INVALID",
                Reason::PasswordNotText,
            )),
        }
    };
    let user = take(&mut map, "userPassword")?;
    let owner = take(&mut map, "ownerPassword")?;
    let secrets = (user.is_some() || owner.is_some()).then(|| Secrets {
        password: None,
        user_password: user,
        owner_password: owner,
    });
    Ok((Value::Object(map), secrets))
}

/// Input order: the `order` param (file IDs from the `fileOrder` widget)
/// when given, otherwise selection order.
fn order_files(params: &Value, file_ids: &[String]) -> CommandResult<Vec<String>> {
    let Some(order) = params.get("order").and_then(Value::as_array) else {
        return Ok(file_ids.to_vec());
    };
    let order: Vec<String> = order
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let mut a = order.clone();
    let mut b = file_ids.to_vec();
    a.sort();
    b.sort();
    if a != b {
        return Err(CommandError::with(
            "E_PARAMS_INVALID",
            Reason::OrderMismatch,
        ));
    }
    Ok(order)
}

/// The selection split into one group per format, in the order the formats
/// first appear.
fn group_by_format(ids: &[String], formats: &[RouterFormat]) -> Vec<(RouterFormat, Vec<String>)> {
    let mut groups: Vec<(RouterFormat, Vec<String>)> = Vec::new();
    for (id, &f) in ids.iter().zip(formats) {
        match groups.iter_mut().find(|g| g.0 == f) {
            Some(g) => g.1.push(id.clone()),
            None => groups.push((f, vec![id.clone()])),
        }
    }
    groups
}

/// An adapter, the files it converts and their formats.
type AdapterGroup = (String, Vec<String>, BTreeSet<RouterFormat>);

/// The selection split by the adapter that converts each format: (adapter,
/// its files, their formats), in the order files were given. `adapter_of` is
/// asked once per format.
fn group_by_adapter(
    ids: &[String],
    formats: &[RouterFormat],
    mut adapter_of: impl FnMut(RouterFormat) -> CommandResult<String>,
) -> CommandResult<Vec<AdapterGroup>> {
    let mut adapters: BTreeMap<RouterFormat, String> = BTreeMap::new();
    let mut groups: Vec<AdapterGroup> = Vec::new();
    for (id, &from) in ids.iter().zip(formats) {
        let adapter = match adapters.get(&from) {
            Some(a) => a.clone(),
            None => {
                let a = adapter_of(from)?;
                adapters.insert(from, a.clone());
                a
            }
        };
        match groups.iter_mut().find(|g| g.0 == adapter) {
            Some(g) => {
                g.1.push(id.clone());
                g.2.insert(from);
            }
            None => groups.push((adapter, vec![id.clone()], BTreeSet::from([from]))),
        }
    }
    Ok(groups)
}

/// Drops the files already in the `target` format: what is left (ids and
/// formats, in order) and how many were dropped.
fn skip_same_format(
    ids: &[String],
    formats: &[RouterFormat],
    target: RouterFormat,
) -> (Vec<String>, Vec<RouterFormat>, usize) {
    let (kept_ids, kept_formats): (Vec<String>, Vec<RouterFormat>) = ids
        .iter()
        .zip(formats)
        .filter(|(_, &f)| f != target)
        .map(|(id, &f)| (id.clone(), f))
        .unzip();
    let skipped = ids.len().saturating_sub(kept_ids.len());
    (kept_ids, kept_formats, skipped)
}

/// `formats` (one per id of `before`) rearranged to follow `after`, a
/// permutation of `before`.
fn formats_in_order(
    before: &[String],
    formats: &[RouterFormat],
    after: &[String],
) -> Vec<RouterFormat> {
    let by_id: BTreeMap<&str, RouterFormat> = before
        .iter()
        .map(String::as_str)
        .zip(formats.iter().copied())
        .collect();
    after
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).copied())
        .collect()
}

/// What a job part takes from its selected files.
struct Collected {
    sources: Vec<InputSource>,
    folders: Vec<Option<String>>,
    password: Option<SecretString>,
    titles: Vec<String>,
}

/// Checks each file in `ids` (known, actionable, accepted) and gathers its
/// staged-input source, folder, title and the part's one password. Pure over
/// the selection map, so it runs under a short lock and is testable.
fn collect_inputs(
    files: &BTreeMap<String, FileEntry>,
    ids: &[String],
    accepts: &dyn Fn(RouterFormat) -> bool,
) -> CommandResult<Collected> {
    let mut sources = Vec::new();
    let mut folders = Vec::new();
    let mut shared: Option<&Zeroizing<String>> = None;
    let mut titles = Vec::new();
    for id in ids {
        let entry = files
            .get(id)
            .ok_or_else(|| CommandError::with("E_UNKNOWN_FILE", Reason::UnknownFile))?;
        let format = RouterFormat::parse(entry.descriptor.format.as_str());
        if !entry.descriptor.actionable || !format.is_some_and(accepts) {
            return Err(CommandError::with("E_INTERNAL", Reason::UnsupportedInput));
        }
        if let Some(p) = entry.password.as_ref() {
            match shared {
                None => shared = Some(p),
                Some(first) if **first != **p => {
                    return Err(CommandError::with(
                        "E_PARAMS_INVALID",
                        Reason::PasswordsDiffer,
                    ));
                }
                Some(_) => {}
            }
        }
        titles.push(stem(&entry.descriptor.display_name));
        folders.push(entry.descriptor.folder.clone());
        sources.push(InputSource {
            path: entry.path.clone(),
            extension: entry.descriptor.format.as_str().to_owned(),
        });
    }
    Ok(Collected {
        sources,
        folders,
        password: shared.map(|p| SecretString::new(p.to_string())),
        titles,
    })
}

/// The core resolves file IDs; the worker only ever sees input order. Drops
/// `order` and fills `fileTitles` when the schema has it and the caller
/// didn't give one.
fn finish_params(params: &mut Value, wants_titles: bool, titles: &[String]) {
    if let Some(obj) = params.as_object_mut() {
        obj.remove("order");
        if wants_titles && !obj.contains_key("fileTitles") {
            obj.insert("fileTitles".into(), json!(titles));
        }
    }
}

/// One part of a job before its inputs are staged.
pub(crate) struct Prepared {
    pub(crate) part: JobPart,
    sources: Vec<InputSource>,
    /// The dropped folder of each source, in the same order.
    folders: Vec<Option<String>>,
    /// The file IDs of the sources, in the same order.
    file_ids: Vec<String>,
    pool: String,
    disk_factor: f64,
}

/// What [`App::start`] stages: every part's inputs end to end, in part order.
struct Flattened {
    sources: Vec<InputSource>,
    folders: Vec<Option<String>>,
    file_ids: Vec<String>,
    parts: Vec<JobPart>,
}

/// Concatenates the parts' inputs. A part's `inputs` index this order, which
/// is grouped by adapter and so differs from the selection order.
fn flatten(parts: Vec<Prepared>) -> Flattened {
    let mut out = Flattened {
        sources: Vec::new(),
        folders: Vec::new(),
        file_ids: Vec::new(),
        parts: Vec::with_capacity(parts.len()),
    };
    for p in parts {
        let mut part = p.part;
        let first = out.sources.len();
        part.inputs = (first..first.saturating_add(p.sources.len())).collect();
        out.sources.extend(p.sources);
        out.folders.extend(p.folders);
        out.file_ids.extend(p.file_ids);
        out.parts.push(part);
    }
    out
}

/// A job in parts queues in the pool of its heaviest part: one that runs
/// LibreOffice or FFmpeg waits for that pool's single slot.
fn strictest_pool<'a>(pools: impl Iterator<Item = &'a str>) -> String {
    const ORDER: [&str; 7] = ["office", "media", "redact", "ocr", "render", "image", "pdf"];
    pools
        .min_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(ORDER.len()))
        .unwrap_or("pdf")
        .to_owned()
}

fn worker_class(m: &converter_router::Manifest) -> &'static str {
    use converter_router::manifest::WorkerClass;
    match m.worker_class {
        WorkerClass::Pdf => "pdf",
        WorkerClass::Image => "image",
        WorkerClass::Render => "render",
        WorkerClass::Office => "office",
        WorkerClass::Ocr => "ocr",
        WorkerClass::Media => "media",
    }
}

/// The plan the UI offered for `from → to`: the matrix's shape for the
/// blueprint's formats, a direct edge otherwise (ADR 0003).
fn best_plan(planner: &Planner, from: RouterFormat, to: RouterFormat) -> Option<Plan> {
    if from.in_blueprint() && to.in_blueprint() {
        matrix::offered(from, to).and_then(|cell| planner.best_for_cell(from, to, cell))
    } else {
        planner.best_direct(from, to)
    }
}

fn stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_owned(),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn reason(e: CommandError) -> String {
        e.reason.unwrap_or_default()
    }

    fn pdf() -> RouterFormat {
        RouterFormat::parse("pdf").unwrap()
    }
    fn png() -> RouterFormat {
        RouterFormat::parse("png").unwrap()
    }
    fn docx() -> RouterFormat {
        RouterFormat::parse("docx").unwrap()
    }

    fn prepared(adapter: &str, files: &[&str]) -> Prepared {
        Prepared {
            part: JobPart {
                adapter_id: adapter.into(),
                inputs: Vec::new(),
                timeout_base_s: 1,
                timeout_s_per_mb: 0.0,
                no_progress_s: 1,
                params: json!({}),
                secrets: None,
                output_format: Format::Pdf,
                recheck: None,
            },
            sources: files
                .iter()
                .map(|f| InputSource {
                    path: PathBuf::from(f),
                    extension: "x".into(),
                })
                .collect(),
            folders: files.iter().map(|_| None).collect(),
            file_ids: ids(files),
            pool: "pdf".into(),
            disk_factor: 1.0,
        }
    }

    #[test]
    fn staged_inputs_are_in_grouped_part_order_not_selection_order() {
        // Selected a, b, c, d; the adapters group them as {a, c} then {b, d}.
        let flat = flatten(vec![prepared("x", &["a", "c"]), prepared("y", &["b", "d"])]);
        assert_eq!(flat.file_ids, ids(&["a", "c", "b", "d"]));
        assert_eq!(flat.parts[0].inputs, vec![0, 1]);
        assert_eq!(flat.parts[1].inputs, vec![2, 3]);
        assert_eq!(flat.sources.len(), flat.file_ids.len());
    }

    #[test]
    fn order_defaults_to_selection_order() {
        let got = order_files(&json!({}), &ids(&["a", "b"])).unwrap();
        assert_eq!(got, ids(&["a", "b"]));
    }

    #[test]
    fn order_param_reorders() {
        let got = order_files(&json!({"order": ["b", "c", "a"]}), &ids(&["a", "b", "c"])).unwrap();
        assert_eq!(got, ids(&["b", "c", "a"]));
    }

    #[test]
    fn order_must_name_the_same_files() {
        for order in [
            json!(["a"]),
            json!(["a", "b", "x"]),
            json!(["a", "a", "b"]),
            json!([1, 2]),
        ] {
            let err = order_files(&json!({ "order": order }), &ids(&["a", "b"])).unwrap_err();
            assert_eq!(err.code, "E_PARAMS_INVALID");
            assert_eq!(reason(err), "orderMismatch");
        }
    }

    #[test]
    fn secrets_leave_the_params() {
        let (params, secrets) = split_secrets(json!({
            "userPassword": "u", "ownerPassword": "o", "keep": 1
        }))
        .unwrap();
        assert_eq!(params, json!({"keep": 1}));
        let s = secrets.unwrap();
        assert_eq!(s.user_password.unwrap().expose(), "u");
        assert_eq!(s.owner_password.unwrap().expose(), "o");
        assert!(s.password.is_none());
    }

    #[test]
    fn no_or_empty_secrets_make_no_secrets() {
        let (params, secrets) =
            split_secrets(json!({"userPassword": "", "ownerPassword": null})).unwrap();
        assert_eq!(params, json!({}));
        assert!(secrets.is_none());
    }

    #[test]
    fn secrets_must_be_text_and_params_an_object() {
        let err = split_secrets(json!({"userPassword": 5})).unwrap_err();
        assert_eq!(reason(err), "passwordNotText");
        let err = split_secrets(json!([1])).unwrap_err();
        assert_eq!(reason(err), "notAnObject");
    }

    #[test]
    fn the_strictest_pool_wins() {
        assert_eq!(
            strictest_pool(["pdf", "office", "image"].into_iter()),
            "office"
        );
        assert_eq!(strictest_pool(["image", "media"].into_iter()), "media");
        assert_eq!(strictest_pool(["render", "pdf"].into_iter()), "render");
        // An unknown pool ranks last.
        assert_eq!(strictest_pool(["mystery", "pdf"].into_iter()), "pdf");
        assert_eq!(strictest_pool(std::iter::empty()), "pdf");
    }

    #[test]
    fn groups_by_format_in_first_seen_order() {
        let g = group_by_format(&ids(&["a", "b", "c", "d"]), &[png(), pdf(), png(), docx()]);
        assert_eq!(
            g,
            vec![
                (png(), ids(&["a", "c"])),
                (pdf(), ids(&["b"])),
                (docx(), ids(&["d"])),
            ]
        );
    }

    #[test]
    fn groups_by_adapter_asking_once_per_format() {
        let mut asked = Vec::new();
        let g = group_by_adapter(
            &ids(&["a", "b", "c", "d"]),
            &[png(), docx(), png(), pdf()],
            |f| {
                asked.push(f);
                Ok(if f == png() || f == pdf() {
                    "images"
                } else {
                    "office"
                }
                .to_owned())
            },
        )
        .unwrap();
        assert_eq!(asked, vec![png(), docx(), pdf()]);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "images");
        assert_eq!(g[0].1, ids(&["a", "c", "d"]));
        assert_eq!(g[0].2, BTreeSet::from([png(), pdf()]));
        assert_eq!(g[1].1, ids(&["b"]));
    }

    #[test]
    fn a_missing_route_fails_the_grouping() {
        let err = group_by_adapter(&ids(&["a"]), &[png()], |_| {
            Err(CommandError::with("E_INTERNAL", Reason::NoRoute))
        })
        .unwrap_err();
        assert_eq!(reason(err), "noRoute");
    }

    #[test]
    fn skipping_same_format_keeps_order_and_counts() {
        let (kept, formats, n) =
            skip_same_format(&ids(&["a", "b", "c"]), &[png(), pdf(), docx()], pdf());
        assert_eq!(
            (kept, formats, n),
            (ids(&["a", "c"]), vec![png(), docx()], 1)
        );
    }

    #[test]
    fn formats_follow_a_reordering() {
        let f = formats_in_order(
            &ids(&["a", "b", "c"]),
            &[png(), pdf(), docx()],
            &ids(&["c", "a", "b"]),
        );
        assert_eq!(f, vec![docx(), png(), pdf()]);
    }

    #[test]
    fn titles_are_added_only_when_wanted_and_order_is_dropped() {
        let mut p = json!({"order": ["a"]});
        finish_params(&mut p, true, &ids(&["x", "y"]));
        assert_eq!(p, json!({"fileTitles": ["x", "y"]}));
        let mut p = json!({"fileTitles": ["mine"]});
        finish_params(&mut p, true, &ids(&["x"]));
        assert_eq!(p, json!({"fileTitles": ["mine"]}));
        let mut p = json!({});
        finish_params(&mut p, false, &ids(&["x"]));
        assert_eq!(p, json!({}));
    }

    #[test]
    fn stems_drop_one_extension() {
        assert_eq!(stem("a.b.pdf"), "a.b");
        assert_eq!(stem(".hidden"), ".hidden");
        assert_eq!(stem("noext"), "noext");
    }

    // collect_inputs, over real sniffed files.

    const PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn selection(
        dir: &std::path::Path,
        names: &[&str],
        passwords: &[Option<&str>],
    ) -> (BTreeMap<String, FileEntry>, Vec<String>) {
        let mut map = BTreeMap::new();
        let mut order = Vec::new();
        for (name, pw) in names.iter().zip(passwords) {
            let path = dir.join(name);
            std::fs::write(&path, PNG_BYTES).unwrap();
            let mut d = converter_sniffer::sniff_path(&path).unwrap();
            d.display_name = (*name).to_owned();
            d.folder = Some("album".into());
            let id = d.file_id.clone();
            let mut e = FileEntry::new(path, d);
            e.password = pw.map(|p| Zeroizing::new(p.to_owned()));
            order.push(id.clone());
            map.insert(id, e);
        }
        (map, order)
    }

    #[test]
    fn collects_sources_titles_folders_in_order() {
        let t = tempfile::tempdir().unwrap();
        let (files, mut order) = selection(t.path(), &["a.png", "b.png"], &[None, None]);
        order.reverse();
        let c = collect_inputs(&files, &order, &|f| f == png()).unwrap();
        assert_eq!(c.titles, ids(&["b", "a"]));
        assert_eq!(c.sources.len(), 2);
        assert!(c.sources[0].path.ends_with("b.png"));
        assert_eq!(c.sources[0].extension, "png");
        assert_eq!(c.folders, vec![Some("album".to_owned()); 2]);
        assert!(c.password.is_none());
    }

    #[test]
    fn one_shared_password_is_collected() {
        let t = tempfile::tempdir().unwrap();
        let (files, order) = selection(
            t.path(),
            &["a.png", "b.png", "c.png"],
            &[None, Some("pw"), Some("pw")],
        );
        let c = collect_inputs(&files, &order, &|_| true).unwrap();
        assert_eq!(c.password.unwrap().expose(), "pw");
    }

    #[test]
    fn different_passwords_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let (files, order) = selection(t.path(), &["a.png", "b.png"], &[Some("one"), Some("two")]);
        let err = collect_inputs(&files, &order, &|_| true).err().unwrap();
        assert_eq!(err.code, "E_PARAMS_INVALID");
        assert_eq!(reason(err), "passwordsDiffer");
    }

    #[test]
    fn unknown_and_unaccepted_files_are_refused() {
        let t = tempfile::tempdir().unwrap();
        let (files, order) = selection(t.path(), &["a.png"], &[None]);
        let err = collect_inputs(&files, &ids(&["nope"]), &|_| true)
            .err()
            .unwrap();
        assert_eq!(err.code, "E_UNKNOWN_FILE");
        let err = collect_inputs(&files, &order, &|f| f == pdf())
            .err()
            .unwrap();
        assert_eq!(reason(err), "unsupportedInput");
    }
}
