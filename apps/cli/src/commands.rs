//! The commands, over `converter-app-core`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use converter_app_core::{with_defaults, App, AppConfig, CommandError, FileResult, InspectInfo};
use converter_jobstore::{FolderLayout, JobEvent, JobId, JobSnapshot, JobState, SaveDestination};
use converter_router::manifest::Kind;
use converter_router::{Format, Targets};
use serde_json::{json, Value};

use crate::args::{build_params, Opts};
use crate::files::{expand, listing, subdirs, Input};
use crate::RUNNER_FLAG;

/// A failure to exit with. `json` is what to print on stdout in `--json`
/// mode (`None`: the command printed its own report already).
pub struct CliError {
    pub code: u8,
    pub message: String,
    pub json: Option<Value>,
}

impl CliError {
    fn new(code: u8, message: impl Into<String>) -> Self {
        CliError {
            code,
            message: message.into(),
            json: None,
        }
    }
    fn usage(message: impl Into<String>) -> Self {
        Self::new(2, message)
    }
    fn failed(message: impl Into<String>) -> Self {
        Self::new(1, message)
    }
    fn sandbox(message: impl Into<String>) -> Self {
        Self::new(3, message)
    }
    /// The report was printed already: exit without another message.
    fn reported(code: u8) -> Self {
        Self::new(code, "")
    }
    fn with_json(mut self, o: &Opts, v: Value) -> Self {
        if o.json {
            self.json = Some(v);
        }
        self
    }
}

type Res<T> = Result<T, CliError>;

pub fn dispatch(o: &Opts) -> Result<(), CliError> {
    let result = match o.command.as_deref() {
        None => Err(CliError::usage("no command given")),
        Some("doctor") => no_args(o).and_then(|_| doctor(o)),
        Some("inspect") => need_files(o).and_then(|_| inspect(o)),
        Some("targets") => need_files(o).and_then(|_| targets(o)),
        Some("convert") => convert(o),
        Some("run") => run(o),
        Some(other) => Err(CliError::usage(format!("unknown command {other:?}"))),
    };
    result.map_err(|mut e| {
        if o.json && e.json.is_none() && !e.message.is_empty() {
            e.json =
                Some(json!({ "ok": false, "error": { "message": e.message, "exit": e.code } }));
        }
        if let Some(j) = &e.json {
            println!("{j}");
        }
        e
    })
}

fn no_args(o: &Opts) -> Res<()> {
    if o.positional.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage("doctor takes no arguments"))
    }
}

fn need_files(o: &Opts) -> Res<()> {
    if o.positional.is_empty() {
        Err(CliError::usage(format!(
            "{} needs at least one file",
            o.command.as_deref().unwrap_or("this command")
        )))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------------ session

/// An open app; shuts down (deleting job folders) when dropped.
struct Session {
    app: App,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

fn describe(e: &CommandError) -> String {
    let mut s = e.code.clone();
    if let Some(r) = &e.reason {
        s.push_str(&format!(" ({r})"));
    }
    for issue in &e.issues {
        if let Some(p) = issue["path"].as_str() {
            let p = if p.is_empty() { "(whole object)" } else { p };
            s.push_str(&format!("\n  invalid parameter {p}"));
        }
    }
    s
}

fn cmd_err(e: CommandError) -> CliError {
    let code = if e.code == "E_SANDBOX_UNAVAILABLE" {
        3
    } else {
        1
    };
    CliError::new(code, describe(&e))
}

type Sink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

fn open(o: &Opts, on_event: Sink) -> Res<Session> {
    let runner = std::env::current_exe().map_err(|e| CliError::failed(e.to_string()))?;
    let mut config = AppConfig::locate(o.resources.clone(), runner, vec![RUNNER_FLAG.into()])
        .map_err(CliError::failed)?;
    config.data_root = o.data_dir.clone();
    let app = App::open(
        &config,
        on_event,
        Arc::new(|_| String::new()),
        Arc::new(|_| {}),
    )
    .map_err(CliError::failed)?;
    Ok(Session { app })
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Opens the app and runs the sandbox self-test; processing needs it.
fn open_ready(o: &Opts, on_event: Sink) -> Res<Session> {
    let s = open(o, on_event)?;
    let report = s.app.self_test(&home());
    if !report.passed {
        let failed: Vec<String> = report
            .attempts
            .iter()
            .filter(|a| !a.passed)
            .map(|a| format!("{}: {}", a.name, a.outcome))
            .collect();
        return Err(CliError::sandbox(format!(
            "the sandbox self-test failed, so nothing was processed ({})",
            failed.join("; ")
        )));
    }
    Ok(s)
}

fn quiet_sink() -> Sink {
    Arc::new(|_| {})
}

// ------------------------------------------------------------------- doctor

fn doctor(o: &Opts) -> Res<()> {
    let s = open(o, quiet_sink())?;
    s.app.self_test(&home());
    let d = s.app.get_diagnostics();
    if o.json {
        println!("{}", serde_json::to_value(&d).unwrap_or(Value::Null));
    } else {
        println!(
            "lfcs {} (protocol {}) on {}",
            d.app_version, d.protocol_version, d.platform
        );
        let verdict = if d.processing_enabled {
            "passed"
        } else {
            "FAILED"
        };
        println!("sandbox: {verdict}");
        for a in d.sandbox.iter().flat_map(|r| &r.attempts) {
            let mark = if a.passed { "ok  " } else { "FAIL" };
            println!("  {mark} {} ({})", a.name, a.outcome);
        }
        let engines = if d.engines.is_empty() {
            "none".to_string()
        } else {
            d.engines.join(", ")
        };
        println!("engines: {engines}");
        println!("adapters ({}): {}", d.adapters.len(), d.adapters.join(", "));
    }
    if d.processing_enabled {
        Ok(())
    } else {
        Err(CliError::reported(3))
    }
}

// ------------------------------------------------------------------- inputs

struct Loaded {
    id: String,
    input: Input,
    descriptor: Value,
    actionable: bool,
}

fn note(o: &Opts, quiet_ok: bool, text: &str) {
    if !(quiet_ok && o.quiet) {
        eprintln!("{text}");
    }
}

/// Sniffs the files. Rejected ones are reported on stderr and left out.
fn load(s: &Session, o: &Opts, skipped: &mut Vec<Value>) -> Res<Vec<Loaded>> {
    let inputs = expand(&o.positional).map_err(CliError::failed)?;
    let paths = inputs.iter().map(|i| i.path.clone()).collect();
    let results: Vec<FileResult> = s.app.add_paths(paths).map_err(cmd_err)?;
    let mut out = Vec::new();
    for (input, r) in inputs.into_iter().zip(results) {
        match r.file {
            Some(d) => out.push(Loaded {
                id: d.file_id.clone(),
                input,
                actionable: d.actionable,
                descriptor: serde_json::to_value(&d).unwrap_or(Value::Null),
            }),
            None => {
                let why = r.error.unwrap_or_else(|| "unreadable".into());
                let name = input.path.display().to_string();
                note(o, input.from_folder, &format!("skipped {name}: {why}"));
                skipped.push(json!({ "file": name, "reason": why }));
            }
        }
    }
    Ok(out)
}

/// The files a job can take; others are skipped with a note.
fn actionable(o: &Opts, loaded: Vec<Loaded>, skipped: &mut Vec<Value>) -> Res<Vec<Loaded>> {
    let (good, bad): (Vec<_>, Vec<_>) = loaded.into_iter().partition(|l| l.actionable);
    for b in bad {
        let name = b.input.path.display().to_string();
        let text = format!("skipped {name}: format not recognised or not supported");
        note(o, b.input.from_folder, &text);
        skipped.push(json!({ "file": name, "reason": "notActionable" }));
    }
    if good.is_empty() {
        return Err(CliError::failed("no usable files"));
    }
    Ok(good)
}

// ------------------------------------------------------------------ inspect

fn inspect(o: &Opts) -> Res<()> {
    let s = open_ready(o, quiet_sink())?;
    let mut skipped = Vec::new();
    let files = load(&s, o, &mut skipped)?;
    let mut report = Vec::new();
    let mut failed = false;
    for f in &files {
        let info: Result<InspectInfo, CommandError> = if f.actionable {
            s.app.inspect_file(&f.id)
        } else {
            Ok(InspectInfo::unknown())
        };
        if o.json {
            let (key, v) = match &info {
                Ok(i) => ("inspect", serde_json::to_value(i).unwrap_or(Value::Null)),
                Err(e) => ("error", json!({ "code": e.code, "reason": e.reason })),
            };
            report.push(json!({
                "file": f.input.path.display().to_string(),
                "descriptor": f.descriptor,
                key: v,
            }));
        } else {
            print_inspect(f, &info);
        }
        failed |= info.is_err();
    }
    if o.json {
        println!(
            "{}",
            json!({ "ok": !failed, "files": report, "skipped": skipped })
        );
    }
    if failed {
        Err(CliError::reported(1))
    } else {
        Ok(())
    }
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("?")
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn print_inspect(f: &Loaded, info: &Result<InspectInfo, CommandError>) {
    let d = &f.descriptor;
    println!("{}", f.input.path.display());
    println!(
        "  format: {} ({} confidence), {}",
        text(d, "format"),
        text(d, "confidence"),
        size(d["sizeBytes"].as_u64().unwrap_or(0))
    );
    if d["extensionMismatch"].as_bool() == Some(true) {
        println!("  the extension doesn't match the contents");
    }
    if d["macroEnabled"].as_bool() == Some(true) {
        println!("  macro-enabled document");
    }
    if d["cloudPlaceholder"].as_bool() == Some(true) {
        println!("  cloud placeholder: not downloaded to this computer");
    }
    if let Some(c) = d["candidates"].as_array().filter(|c| !c.is_empty()) {
        let names: Vec<&str> = c.iter().filter_map(Value::as_str).collect();
        println!("  could also be: {}", names.join(", "));
    }
    match info {
        Err(e) => println!("  inspect failed: {}", describe(e)),
        Ok(i) => {
            let v = serde_json::to_value(i).unwrap_or(Value::Null);
            if i.pages > 0 {
                println!("  pages: {}", i.pages);
            }
            println!("  encryption: {}", i.encryption);
            if let Some(sig) = v["signed"].as_bool() {
                println!("  signed: {sig}");
            }
            if !v["media"].is_null() {
                println!("  media: {}", v["media"]);
            }
            for w in &i.warnings {
                println!("  warning: {w}");
            }
            for fi in v["findings"].as_array().into_iter().flatten() {
                match fi["detail"].as_str() {
                    Some(detail) => println!("  finding: {}: {detail}", text(fi, "kind")),
                    None => println!("  finding: {}", text(fi, "kind")),
                }
            }
        }
    }
}

// ------------------------------------------------------------------ targets

fn why(u: &Value) -> String {
    match u["reason"].as_str().unwrap_or("unavailable") {
        "platformUnsupported" => "not supported on this platform".into(),
        "engineMissing" => format!(
            "needs {} installed",
            u["engine"].as_str().unwrap_or("an engine")
        ),
        "tooFewFiles" => format!("needs at least {} files", u["min"]),
        "tooManyFiles" => format!("takes at most {} files", u["max"]),
        other => other.into(),
    }
}

fn unavailable<T: serde::Serialize>(u: &Option<T>) -> String {
    match u {
        None => String::new(),
        Some(u) => {
            let v = serde_json::to_value(u).unwrap_or(Value::Null);
            format!("  [unavailable: {}]", why(&v))
        }
    }
}

/// A unit-like enum's serialized name, such as a job state or a tier.
fn name_of<T: serde::Serialize>(t: &T) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn targets(o: &Opts) -> Res<()> {
    let s = open_ready(o, quiet_sink())?;
    let mut skipped = Vec::new();
    let files = actionable(o, load(&s, o, &mut skipped)?, &mut skipped)?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let t = s.app.get_targets(&ids).map_err(cmd_err)?;
    if o.json {
        let mut v = serde_json::to_value(&t).unwrap_or(Value::Null);
        v["ok"] = json!(true);
        v["skipped"] = json!(skipped);
        println!("{v}");
        return Ok(());
    }
    println!("{} file(s)", files.len());
    if !t.operations.is_empty() {
        println!("tools (lfcs run TOOL ...):");
        for op in &t.operations {
            println!(
                "  {:<18} {}{}",
                op.adapter,
                op.title,
                unavailable(&op.unavailable)
            );
        }
    }
    for g in &t.groups {
        println!(
            "convert {} ({} file(s)) with --to:",
            g.format.as_str(),
            g.files
        );
        if g.conversions.is_empty() {
            println!("  nothing");
        }
        for c in &g.conversions {
            let best = if c.best_effort { ", best effort" } else { "" };
            println!(
                "  {:<8} tier {}{best}{}",
                c.to.as_str(),
                name_of(&c.tier),
                unavailable(&c.unavailable)
            );
        }
    }
    if !t.shared.is_empty() {
        let names: Vec<&str> = t.shared.iter().map(|c| c.to.as_str()).collect();
        println!("every selected file can go to: {}", names.join(", "));
    }
    Ok(())
}

// ------------------------------------------------------------------ convert

fn convert(o: &Opts) -> Res<()> {
    need_files(o)?;
    let to =
        o.to.as_deref()
            .ok_or_else(|| CliError::usage("convert needs --to FORMAT"))?;
    let target =
        Format::parse(to).ok_or_else(|| CliError::usage(format!("unknown format {to:?}")))?;
    let user = build_params(o.params.as_deref(), &o.sets).map_err(CliError::usage)?;

    let s = open_ready(o, progress_sink(o))?;
    let mut skipped = Vec::new();
    let files = actionable(o, load(&s, o, &mut skipped)?, &mut skipped)?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let t = s.app.get_targets(&ids).map_err(cmd_err)?;
    let adapters = conversion_adapters(&t, target)?;

    // With one adapter its schema defaults apply; with several (a mixed
    // selection) each adapter runs with its own, as in the app.
    let params = match adapters.as_slice() {
        [one] => {
            let schema = s
                .app
                .registry()
                .get(one)
                .map(|e| e.params_schema.clone())
                .unwrap_or(Value::Null);
            with_defaults(&schema, Value::Object(user))
        }
        _ => Value::Object(user),
    };
    let job = s
        .app
        .create_conversion(&ids, target.as_str(), params)
        .map_err(cmd_err)?;

    let dir = o.out.clone().unwrap_or_else(|| PathBuf::from("."));
    let layouts: Vec<Option<String>> = files.iter().map(|f| f.input.layout.clone()).collect();
    let keep = o.keep_folders && layouts.iter().any(Option::is_some);
    let dest = Dest::Folder {
        dir,
        layout: keep.then_some(layouts),
    };
    finish(&s, o, &job, dest, skipped)
}

/// The adapter for each selected format, or why `target` can't be reached.
fn conversion_adapters(t: &Targets, target: Format) -> Res<Vec<String>> {
    let mut adapters = BTreeSet::new();
    for g in &t.groups {
        let Some(c) = g.conversions.iter().find(|c| c.to == target) else {
            return Err(CliError::failed(format!(
                "no conversion from {} to {} (see `lfcs targets`)",
                g.format.as_str(),
                target.as_str()
            )));
        };
        if let Some(u) = &c.unavailable {
            let reason = why(&serde_json::to_value(u).unwrap_or(Value::Null));
            return Err(CliError::failed(format!(
                "{} to {} is unavailable: {reason}",
                g.format.as_str(),
                target.as_str()
            )));
        }
        if let Some(h) = c.plan.as_ref().and_then(|p| p.hops.first()) {
            adapters.insert(h.adapter.clone());
        }
    }
    Ok(adapters.into_iter().collect())
}

// ---------------------------------------------------------------------- run

fn run(o: &Opts) -> Res<()> {
    let Some(tool) = o.positional.first().cloned() else {
        return Err(CliError::usage("run needs a tool name and files"));
    };
    if o.positional.len() < 2 {
        return Err(CliError::usage("run needs at least one file"));
    }
    let user = build_params(o.params.as_deref(), &o.sets).map_err(CliError::usage)?;
    let files_only = Opts {
        positional: o.positional[1..].to_vec(),
        quiet: o.quiet,
        ..Opts::default()
    };

    let s = open_ready(o, progress_sink(o))?;
    let registry = s.app.registry();
    let entry = registry
        .get(&tool)
        .filter(|e| e.manifest.kind == Kind::Operation)
        .ok_or_else(|| {
            let names: Vec<&str> = registry
                .adapters
                .iter()
                .filter(|a| a.manifest.kind == Kind::Operation && !a.manifest.dev_only)
                .map(|a| a.manifest.id.as_str())
                .collect();
            CliError::usage(format!(
                "unknown tool {tool:?}; tools: {}",
                names.join(", ")
            ))
        })?;
    let params = with_defaults(&entry.params_schema, Value::Object(user));

    let mut skipped = Vec::new();
    let files = actionable(o, load(&s, &files_only, &mut skipped)?, &mut skipped)?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let job = s.app.create_job(&tool, &ids, params).map_err(cmd_err)?;
    let dest = match &o.out {
        Some(p) => Dest::Auto(p.clone()),
        None => Dest::Folder {
            dir: PathBuf::from("."),
            layout: None,
        },
    };
    finish(&s, o, &job, dest, skipped)
}

// ------------------------------------------------------------- job and save

enum Dest {
    /// A folder, optionally keeping the layout of given folders (one entry
    /// per input, in the order the files were given).
    Folder {
        dir: PathBuf,
        layout: Option<Vec<Option<String>>>,
    },
    /// `run --out PATH`: a file name for a single result, else a folder.
    Auto(PathBuf),
}

fn progress_sink(o: &Opts) -> Sink {
    if o.quiet {
        return quiet_sink();
    }
    let last = Mutex::new(String::new());
    Arc::new(move |e: &JobEvent| {
        let mut line = name_of(&e.state);
        if let Some(st) = &e.stage {
            line.push_str(&format!(": {st}"));
        }
        if let Some(p) = &e.progress {
            match p.total {
                Some(t) => line.push_str(&format!(" {}/{t}", p.done)),
                None => line.push_str(&format!(" {}", p.done)),
            }
        }
        let mut last = last.lock().unwrap_or_else(|p| p.into_inner());
        if *last != line {
            eprintln!("[{line}]");
            *last = line;
        }
    })
}

fn wait_terminal(app: &App, job: &JobId) -> Res<JobSnapshot> {
    loop {
        let snap = app
            .get_snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == job)
            .ok_or_else(|| CliError::failed("the job disappeared"))?;
        if snap.state.is_terminal() {
            return Ok(snap);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn warnings_json(j: &JobSnapshot) -> Vec<Value> {
    j.warnings
        .iter()
        .map(|w| json!({ "code": w.code, "detail": w.detail }))
        .collect()
}

fn finish(s: &Session, o: &Opts, job: &JobId, dest: Dest, skipped: Vec<Value>) -> Res<()> {
    let snap = wait_terminal(&s.app, job)?;
    for w in &snap.warnings {
        match &w.detail {
            Some(d) => eprintln!("warning: {}: {d}", w.code),
            None => eprintln!("warning: {}", w.code),
        }
    }
    let state = name_of(&snap.state);
    let usable =
        matches!(snap.state, JobState::Succeeded | JobState::Partial) && !snap.outputs.is_empty();
    if !usable {
        let _ = s.app.discard_job(job);
        let message = match &snap.error {
            Some(e) => format!("the job failed: {}", e.code),
            None => format!("the job ended {state} with no results"),
        };
        let report = json!({
            "ok": false, "state": state,
            "error": snap.error.as_ref().map(|e| json!({ "code": e.code, "retryable": e.retryable })),
            "warnings": warnings_json(&snap), "skipped": skipped,
        });
        return Err(CliError::failed(message).with_json(o, report));
    }

    let (dest, out_dir, watch) = match resolve(o, dest, snap.outputs.len()) {
        Ok(d) => d,
        Err(e) => {
            let _ = s.app.discard_job(job);
            return Err(e);
        }
    };
    if let Ok(check) = s.app.check_save(job, &dest) {
        if check.sync_provider.is_some() {
            eprintln!("note: the destination is in a cloud-synced folder");
        }
    }
    let before = out_dir.as_ref().map(|d| listing(d, &watch));
    let before_dirs = out_dir.as_deref().map(subdirs).unwrap_or_default();
    let saved_n = match s.app.save_outputs(job, &dest, false) {
        Ok(n) => n,
        Err(e) => {
            let message = if e.code == "E_CONFIRM_OVERWRITE" {
                "refusing to overwrite an input file".to_string()
            } else {
                describe(&e)
            };
            let _ = s.app.discard_job(job);
            return Err(CliError::failed(message));
        }
    };
    let saved: Vec<PathBuf> = match (&dest, &out_dir, before) {
        (SaveDestination::File(p), _, _) => vec![p.clone()],
        (_, Some(dir), Some(before)) => {
            let mut descend = watch.clone();
            descend.extend(subdirs(dir).difference(&before_dirs).cloned());
            listing(dir, &descend)
                .difference(&before)
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    };

    let partial = snap.state == JobState::Partial;
    if o.json {
        println!(
            "{}",
            json!({
                "ok": !partial, "state": state, "savedCount": saved_n,
                "saved": saved.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "outDir": out_dir.as_ref().map(|d| d.display().to_string()),
                "warnings": warnings_json(&snap), "skipped": skipped,
            })
        );
    } else {
        for p in &saved {
            println!("{}", p.display());
        }
        if !o.quiet {
            match &out_dir {
                Some(d) => eprintln!("saved {saved_n} file(s) to {}", d.display()),
                None => eprintln!("saved {saved_n} file(s)"),
            }
        }
    }
    if partial {
        eprintln!("only some files could be converted (see the warnings)");
        Err(CliError::reported(1))
    } else {
        Ok(())
    }
}

/// The save destination, the folder to look in for what was written, and
/// the top folders of a kept layout to look into.
fn resolve(
    o: &Opts,
    dest: Dest,
    outputs: usize,
) -> Res<(SaveDestination, Option<PathBuf>, BTreeSet<String>)> {
    let make = |dir: &PathBuf| {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::failed(format!("{}: {e}", dir.display())))
    };
    match dest {
        Dest::Folder { dir, layout } => {
            make(&dir)?;
            let watch: BTreeSet<String> = layout
                .iter()
                .flatten()
                .flatten()
                .filter_map(|l| l.split('/').next().map(str::to_owned))
                .collect();
            let d = match layout {
                Some(inputs) => SaveDestination::FolderKeepingLayout(FolderLayout {
                    dir: dir.clone(),
                    inputs,
                }),
                None => SaveDestination::Folder(dir.clone()),
            };
            Ok((d, Some(dir), watch))
        }
        Dest::Auto(path) => {
            let as_file =
                outputs == 1 && !path.is_dir() && !path.to_string_lossy().ends_with(['/', '\\']);
            if !as_file {
                make(&path)?;
                return Ok((
                    SaveDestination::Folder(path.clone()),
                    Some(path),
                    BTreeSet::new(),
                ));
            }
            if path.exists() && !o.force {
                return Err(CliError::usage(format!(
                    "{} exists; pass --force to replace it, or give a folder",
                    path.display()
                )));
            }
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                make(&parent.to_path_buf())?;
            }
            Ok((SaveDestination::File(path), None, BTreeSet::new()))
        }
    }
}
