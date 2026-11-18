//! The commands, over `converter-app-core`.

use std::collections::BTreeSet;
use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use converter_app_core::{with_defaults, App, AppConfig, CommandError, InspectInfo};
use converter_jobstore::{JobEvent, JobId, JobSnapshot, JobState, SaveDestination};
use converter_router::manifest::Kind;
use converter_router::{Format, Targets};
use serde_json::{json, Value};

use crate::args::{build_params, Opts, UsageError};
use crate::secret::{self, Secret};
use crate::signal;
use crate::text as words;
use crate::RUNNER_FLAG;

/// A failure to exit with. In `--json` mode it prints as
/// `{ok: false, error: {code, reason, message}}`, the same everywhere
/// (audit U20), unless the command has a fuller report of its own in `json`.
/// `code` is an `E_*` code, `reason` a camelCase name for the cause, and
/// `message` text for people; none holds file contents.
#[derive(Debug)]
pub struct CliError(Box<Failure>);

/// Boxed so a `Result` carrying it stays small.
#[derive(Debug)]
pub struct Failure {
    pub exit: u8,
    pub code: String,
    pub reason: String,
    pub message: String,
    pub json: Option<Value>,
}

impl std::ops::Deref for CliError {
    type Target = Failure;
    fn deref(&self) -> &Failure {
        &self.0
    }
}

impl std::ops::DerefMut for CliError {
    fn deref_mut(&mut self) -> &mut Failure {
        &mut self.0
    }
}

impl CliError {
    fn new(exit: u8, code: &str, reason: &str, message: impl Into<String>) -> Self {
        CliError(Box::new(Failure {
            exit,
            code: code.into(),
            reason: reason.into(),
            message: message.into(),
            json: None,
        }))
    }
    fn usage(reason: &str, message: impl Into<String>) -> Self {
        Self::new(2, "E_USAGE", reason, message)
    }
    fn failed(code: &str, reason: &str, message: impl Into<String>) -> Self {
        Self::new(1, code, reason, message)
    }
    fn sandbox(message: impl Into<String>) -> Self {
        Self::new(3, "E_SANDBOX_UNAVAILABLE", "selfTestFailed", message)
    }
    /// Ctrl-C: nothing was saved.
    fn cancelled() -> Self {
        let message = words::message("E_CANCELLED", None);
        Self::new(
            signal::EXIT_INTERRUPTED,
            "E_CANCELLED",
            "interrupted",
            message,
        )
    }
    /// The report was printed already: exit without another message.
    fn reported(exit: u8) -> Self {
        Self::new(exit, "", "", "")
    }
    fn with_json(mut self, o: &Opts, v: Value) -> Self {
        if o.json {
            self.json = Some(v);
        }
        self
    }
    fn error_json(&self) -> Value {
        error_json(&self.code, &self.reason, &self.message)
    }
}

impl From<UsageError> for CliError {
    fn from(e: UsageError) -> Self {
        CliError::usage(e.reason, e.message)
    }
}

fn error_json(code: &str, reason: &str, message: &str) -> Value {
    json!({ "code": code, "reason": reason, "message": message })
}

type Res<T> = Result<T, CliError>;

pub fn dispatch(o: &Opts) -> Result<(), CliError> {
    match o.command.as_deref() {
        None => Err(CliError::usage("noCommand", "no command given")),
        Some("doctor") => no_args(o).and_then(|_| doctor(o)),
        Some("inspect") => need_files(o).and_then(|_| inspect(o)),
        Some("targets") => need_files(o).and_then(|_| targets(o)),
        Some("convert") => convert(o),
        Some("run") => run(o),
        Some(other) => Err(CliError::usage(
            "unknownCommand",
            format!("unknown command {other:?}"),
        )),
    }
}

/// Prints a failure (JSON on stdout when asked, text on stderr) and gives
/// the exit code.
pub fn report(e: CliError, json: bool) -> ExitCode {
    if json {
        if let Some(j) = &e.json {
            println!("{j}");
        } else if !e.message.is_empty() {
            println!("{}", json!({ "ok": false, "error": e.error_json() }));
        }
    }
    if !e.message.is_empty() {
        eprintln!("lfcs: {}", e.message);
        if e.exit == 2 {
            eprintln!("try `lfcs --help`");
        } else if !e.code.is_empty() {
            eprintln!("  ({}, {})", e.code, e.reason);
        }
    }
    ExitCode::from(e.exit)
}

fn no_args(o: &Opts) -> Res<()> {
    if o.positional.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(
            "unexpectedArguments",
            "doctor takes no arguments",
        ))
    }
}

fn need_files(o: &Opts) -> Res<()> {
    if o.positional.is_empty() {
        Err(CliError::usage(
            "noFiles",
            format!(
                "{} needs at least one file",
                o.command.as_deref().unwrap_or("this command")
            ),
        ))
    } else {
        Ok(())
    }
}

fn check_interrupt() -> Res<()> {
    if signal::interrupted() {
        Err(CliError::cancelled())
    } else {
        Ok(())
    }
}

/// `E_INPUT_CORRUPT` as `inputCorrupt`, for a code that came without a reason.
fn camel_of_code(code: &str) -> String {
    let mut out = String::new();
    for (i, part) in code.trim_start_matches("E_").split('_').enumerate() {
        let lower = part.to_ascii_lowercase();
        let mut chars = lower.chars();
        match chars.next() {
            Some(c) if i == 0 => out.push(c),
            Some(c) => out.extend(c.to_uppercase()),
            None => {}
        }
        out.extend(chars);
    }
    out
}

// ------------------------------------------------------------------ session

/// An open app; shuts down (deleting job folders) when dropped.
struct Session {
    app: App,
    /// For encrypted inputs, and as the password a tool sets (pdf-protect).
    /// Wiped when the session ends.
    password: Option<Secret>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.app.shutdown();
    }
}

fn describe(code: &str, reason: Option<&str>, issues: &[Value]) -> String {
    let mut s = words::message(code, reason);
    let mut wants_password = code == "E_PASSWORD_REQUIRED" || code == "E_PASSWORD_WRONG";
    for issue in issues {
        if let Some(p) = issue["path"].as_str() {
            wants_password |= p.to_ascii_lowercase().contains("password");
            let p = if p.is_empty() { "(whole object)" } else { p };
            s.push_str(&format!("\n  invalid setting {p}"));
        }
    }
    if wants_password {
        s.push_str("\n  Give the password with --password-stdin or the LFCS_PASSWORD environment variable.");
    }
    s
}

fn cmd_err(e: CommandError) -> CliError {
    let exit = if e.code == "E_SANDBOX_UNAVAILABLE" {
        3
    } else {
        1
    };
    let reason = e.reason.clone().unwrap_or_else(|| camel_of_code(&e.code));
    let message = describe(&e.code, e.reason.as_deref(), &e.issues);
    CliError::new(exit, &e.code, &reason, message)
}

type Sink = Arc<dyn Fn(&JobEvent) + Send + Sync>;

fn open(o: &Opts, on_event: Sink) -> Res<Session> {
    let not_started = |m: String| CliError::failed("E_INSTALL_CORRUPT", "appNotStarted", m);
    let runner = std::env::current_exe().map_err(|e| not_started(e.to_string()))?;
    let args = vec![RUNNER_FLAG.into()];
    // `locate` reads LFCS_DEV; the flag is the same switch.
    let mut config = if o.dev {
        AppConfig::locate_with(o.resources.clone(), runner, args, true)
    } else {
        AppConfig::locate(o.resources.clone(), runner, args)
    }
    .map_err(not_started)?;
    config.data_root = o.data_dir.clone();
    let app = App::open(
        &config,
        on_event,
        Arc::new(|_| String::new()),
        Arc::new(|_| {}),
    )
    .map_err(not_started)?;
    Ok(Session {
        app,
        password: None,
    })
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The password from the first line of standard input (`--password-stdin`)
/// or `LFCS_PASSWORD`, never from an argument (audit U19). On a terminal,
/// `--password-stdin` asks for it with the typing hidden. The variable is
/// removed once read, so nothing started later inherits it.
fn read_password(o: &Opts) -> Res<Option<Secret>> {
    let from_env = std::env::var("LFCS_PASSWORD").ok().map(Secret::new);
    if from_env.is_some() {
        std::env::remove_var("LFCS_PASSWORD");
    }
    if o.password_stdin {
        let stdin = std::io::stdin();
        let line = if stdin.is_terminal() {
            prompt_hidden()
        } else {
            Ok(first_line(stdin.lock()))
        };
        return match line {
            Ok(Some(pw)) => Ok(Some(pw)),
            Err(PromptError::Interrupted) => Err(CliError::cancelled()),
            Err(PromptError::Unsupported) => Err(CliError::usage(
                "noPasswordPrompt",
                "--password-stdin: this terminal can't hide typing; pipe the password in or set LFCS_PASSWORD",
            )),
            Ok(None) | Err(PromptError::Failed) => Err(CliError::usage(
                "noPassword",
                "--password-stdin: standard input had no password on its first line",
            )),
        };
    }
    Ok(from_env.filter(|p| !p.expose().is_empty()))
}

/// The first line of `input` without its line end; `None` if there is none or
/// it is empty. The raw line is wiped once the trimmed copy is made.
fn first_line(mut input: impl BufRead) -> Option<Secret> {
    let mut line = String::new();
    let read = input.read_line(&mut line);
    let trimmed = line.trim_end_matches(['\n', '\r']).to_owned();
    secret::wipe_string(&mut line);
    match read {
        Ok(n) if n > 0 && !trimmed.is_empty() => Some(Secret::new(trimmed)),
        _ => {
            let mut t = trimmed;
            secret::wipe_string(&mut t);
            None
        }
    }
}

// Only the terminal prompt (Unix) raises the first and last.
#[cfg_attr(not(unix), allow(dead_code))]
enum PromptError {
    Interrupted,
    /// No way to hide typing on this platform.
    Unsupported,
    Failed,
}

/// Asks on the terminal with echo off. Ctrl-C arrives as a character (the
/// signal is switched off while typing), because the handler only sets a
/// flag and a blocked read would never see it.
#[cfg(unix)]
fn prompt_hidden() -> Result<Option<Secret>, PromptError> {
    use std::io::Write;

    // Restores the terminal on every way out.
    struct Restore(libc::termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &self.0) };
        }
    }
    let mut old: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut old) } != 0 {
        return Err(PromptError::Unsupported);
    }
    let mut quiet = old;
    quiet.c_lflag &= !(libc::ECHO | libc::ISIG);
    let restore = Restore(old);
    if unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &quiet) } != 0 {
        return Err(PromptError::Unsupported);
    }
    eprint!("Password: ");
    let _ = std::io::stderr().flush();
    // Unbuffered reads of fd 0, so no library buffer keeps a copy.
    let mut bytes = Vec::new();
    let outcome = loop {
        let mut b = 0u8;
        let n = unsafe { libc::read(0, (&mut b as *mut u8).cast(), 1) };
        match n {
            1 if b == b'\n' || b == b'\r' => break Ok(()),
            1 if b == 0x03 => break Err(PromptError::Interrupted),
            1 => bytes.push(b),
            0 => break Ok(()),
            _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {}
            _ => break Err(PromptError::Failed),
        }
    };
    drop(restore);
    eprintln!();
    let text = String::from_utf8(bytes);
    outcome?;
    match text {
        Ok(t) if !t.is_empty() => Ok(Some(Secret::new(t))),
        Ok(_) => Ok(None),
        Err(e) => {
            let mut raw = e.into_bytes();
            secret::wipe(&mut raw);
            Err(PromptError::Failed)
        }
    }
}

#[cfg(not(unix))]
fn prompt_hidden() -> Result<Option<Secret>, PromptError> {
    Err(PromptError::Unsupported)
}

/// Opens the app and runs the sandbox self-test; processing needs it.
fn open_ready(o: &Opts, on_event: Sink) -> Res<Session> {
    let password = read_password(o)?;
    let mut s = open(o, on_event)?;
    s.password = password;
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
        let mut v = serde_json::to_value(&d).unwrap_or(Value::Null);
        v["ok"] = json!(d.processing_enabled);
        if !d.processing_enabled {
            let message = words::message("E_SANDBOX_UNAVAILABLE", None);
            v["error"] = error_json("E_SANDBOX_UNAVAILABLE", "selfTestFailed", &message);
        }
        println!("{v}");
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
    /// What to call it in output: the path as typed for a file, the folder
    /// and file name for one found in a folder.
    name: String,
    descriptor: Value,
    actionable: bool,
    /// Found by walking a folder (so unrecognised files are skipped quietly).
    from_folder: bool,
}

/// What was left out, for the report.
#[derive(Default)]
struct Notes {
    skipped: Vec<Value>,
    /// Files `--skip-same` left out because they were already in the target format.
    skipped_same: usize,
    /// Folders with more files than fit under the 200-file limit.
    folder_truncated: Vec<Value>,
}

fn note(o: &Opts, quiet_ok: bool, text: &str) {
    if !(quiet_ok && o.quiet) {
        eprintln!("{text}");
    }
}

fn skip(o: &Opts, notes: &mut Notes, quiet_ok: bool, name: &str, reason: &str, message: &str) {
    note(o, quiet_ok, &format!("skipped {name}: {message}"));
    notes
        .skipped
        .push(json!({ "file": name, "reason": reason, "message": message }));
}

/// Sniffs the files; a folder is walked by the core (`intake::walk`, the same
/// rules and the same 200-file limit as the app). Rejected files are
/// reported on stderr and left out; files that didn't fit are reported as
/// `folderTruncated`.
fn load(s: &Session, o: &Opts, notes: &mut Notes) -> Res<Vec<Loaded>> {
    let mut out = Vec::new();
    for arg in &o.positional {
        check_interrupt()?;
        let path = PathBuf::from(arg);
        let from_folder = path.is_dir();
        for r in s.app.add_paths(vec![path]).map_err(cmd_err)? {
            if r.error.as_deref() == Some("folderTruncated") {
                let count = r.count.unwrap_or(0);
                note(
                    o,
                    false,
                    &format!("note: {}", words::folder_truncated(arg, count)),
                );
                notes
                    .folder_truncated
                    .push(json!({ "folder": arg, "count": count }));
                continue;
            }
            let Some(d) = r.file else {
                let why = r.error.unwrap_or_else(|| "unreadable".into());
                let message = words::sniff_error(&why).unwrap_or("The file couldn't be read.");
                let name = if from_folder {
                    r.display_name
                } else {
                    arg.clone()
                };
                skip(o, notes, from_folder, &name, &why, message);
                continue;
            };
            let name = match &d.folder {
                Some(f) => format!("{f}/{}", d.display_name),
                None => arg.clone(),
            };
            let encrypted = serde_json::to_value(d.encryption_hint).ok() == Some(json!("maybe"));
            if let (true, Some(pw)) = (encrypted, &s.password) {
                // Any failure leaves the file out with its reason, not only a
                // wrong password: carrying on would fail later with a vaguer
                // "password required".
                if let Err(e) = s.app.submit_password(&d.file_id, pw.expose().to_owned()) {
                    let (reason, message) = password_failure(&e);
                    skip(o, notes, false, &name, &reason, &message);
                    continue;
                }
            }
            out.push(Loaded {
                id: d.file_id.clone(),
                name,
                actionable: d.actionable,
                descriptor: serde_json::to_value(&d).unwrap_or(Value::Null),
                from_folder,
            });
        }
    }
    Ok(out)
}

/// Why a stored password couldn't be applied to a file: the `reason` for
/// `skipped` and text for people. No file contents or the password.
fn password_failure(e: &CommandError) -> (String, String) {
    let reason = if e.code == "E_PASSWORD_WRONG" {
        "passwordWrong".to_owned()
    } else {
        e.reason.clone().unwrap_or_else(|| camel_of_code(&e.code))
    };
    (reason, describe(&e.code, e.reason.as_deref(), &[]))
}

/// The files a job can take; others are skipped with a note.
fn actionable(o: &Opts, loaded: Vec<Loaded>, notes: &mut Notes) -> Res<Vec<Loaded>> {
    let (good, bad): (Vec<_>, Vec<_>) = loaded.into_iter().partition(|l| l.actionable);
    for b in bad {
        skip(
            o,
            notes,
            b.from_folder,
            &b.name,
            "notActionable",
            "format not recognised or not supported",
        );
    }
    if good.is_empty() {
        return Err(CliError::failed(
            "E_NO_USABLE_FILES",
            "noUsableFiles",
            "no usable files",
        ));
    }
    Ok(good)
}

// ------------------------------------------------------------------ inspect

fn inspect(o: &Opts) -> Res<()> {
    let s = open_ready(o, quiet_sink())?;
    let mut notes = Notes::default();
    let files = load(&s, o, &mut notes)?;
    let mut report = Vec::new();
    let mut first_error: Option<Value> = None;
    for f in &files {
        check_interrupt()?;
        let info: Result<InspectInfo, CommandError> = if f.actionable {
            s.app.inspect_file(&f.id)
        } else {
            Ok(InspectInfo::unknown())
        };
        if o.json {
            let (key, v) = match &info {
                Ok(i) => ("inspect", serde_json::to_value(i).unwrap_or(Value::Null)),
                Err(e) => {
                    let e = cmd_err(e.clone());
                    first_error.get_or_insert_with(|| e.error_json());
                    ("error", e.error_json())
                }
            };
            report.push(json!({
                "file": f.name,
                "descriptor": f.descriptor,
                key: v,
            }));
        } else {
            print_inspect(f, &info);
        }
        if let Err(e) = &info {
            first_error.get_or_insert_with(|| cmd_err(e.clone()).error_json());
        }
    }
    let failed = first_error.is_some();
    if o.json {
        let mut v = json!({
            "ok": !failed, "files": report,
            "skipped": notes.skipped, "folderTruncated": notes.folder_truncated,
        });
        // The first failure at the top, so the failure shape is the same as
        // for every other command; each file's own is in `files`.
        if let Some(e) = first_error {
            v["error"] = e;
        }
        println!("{v}");
    }
    if failed {
        Err(CliError::reported(1))
    } else {
        Ok(())
    }
}

fn field<'a>(v: &'a Value, key: &str) -> &'a str {
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
    println!("{}", f.name);
    println!(
        "  format: {} ({} confidence), {}",
        field(d, "format"),
        field(d, "confidence"),
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
        Err(e) => println!("  inspect failed: {}", cmd_err(e.clone()).message),
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
                println!("  warning: {} ({w})", words::warning(w));
            }
            for fi in v["findings"].as_array().into_iter().flatten() {
                match fi["detail"].as_str() {
                    Some(detail) => println!("  finding: {}: {detail}", field(fi, "kind")),
                    None => println!("  finding: {}", field(fi, "kind")),
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
    let mut notes = Notes::default();
    let loaded = load(&s, o, &mut notes)?;
    let files = actionable(o, loaded, &mut notes)?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let t = s.app.get_targets(&ids).map_err(cmd_err)?;
    if o.json {
        let mut v = serde_json::to_value(&t).unwrap_or(Value::Null);
        v["ok"] = json!(true);
        v["skipped"] = json!(notes.skipped);
        v["folderTruncated"] = json!(notes.folder_truncated);
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
            .ok_or_else(|| CliError::usage("missingTarget", "convert needs --to FORMAT"))?;
    let target = Format::parse(to)
        .ok_or_else(|| CliError::usage("unknownFormat", format!("unknown format {to:?}")))?;
    let user = build_params(o.params.as_deref(), &o.sets)?;

    let s = open_ready(o, progress_sink(o))?;
    let mut notes = Notes::default();
    let loaded = load(&s, o, &mut notes)?;
    let files = actionable(o, loaded, &mut notes)?;
    check_interrupt()?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let t = s.app.get_targets(&ids).map_err(cmd_err)?;
    let adapters = conversion_adapters(&t, target, o.skip_same)?;

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
    let params = with_skip_same(params, o.skip_same);
    let (job, skipped_same) = s
        .app
        .create_conversion_counted(&ids, target.as_str(), params)
        .map_err(cmd_err)?;
    notes.skipped_same = skipped_same;
    if skipped_same > 0 {
        note(
            o,
            true,
            &format!(
                "skipped {skipped_same} file(s) already in {}",
                target.as_str()
            ),
        );
    }

    let dir = o.out.clone().unwrap_or_else(|| PathBuf::from("."));
    finish(&s, o, &job, Dest::Folder(dir), notes)
}

/// `--skip-same` is the core's `skipSame` parameter, as in the app.
fn with_skip_same(params: Value, skip_same: bool) -> Value {
    match params {
        Value::Object(mut m) if skip_same => {
            m.insert("skipSame".into(), Value::Bool(true));
            Value::Object(m)
        }
        other => other,
    }
}

/// The adapter for each selected format, or why `target` can't be reached.
/// With `skip_same`, files already in `target` need no route: the core
/// leaves them out.
fn conversion_adapters(t: &Targets, target: Format, skip_same: bool) -> Res<Vec<String>> {
    let mut adapters = BTreeSet::new();
    for g in &t.groups {
        if skip_same && g.format == target {
            continue;
        }
        let Some(c) = g.conversions.iter().find(|c| c.to == target) else {
            return Err(CliError::failed(
                "E_NO_ROUTE",
                "noRoute",
                format!(
                    "no conversion from {} to {} (see `lfcs targets`)",
                    g.format.as_str(),
                    target.as_str()
                ),
            ));
        };
        if let Some(u) = &c.unavailable {
            let reason = why(&serde_json::to_value(u).unwrap_or(Value::Null));
            return Err(CliError::failed(
                "E_NO_ROUTE",
                "conversionUnavailable",
                format!(
                    "{} to {} is unavailable: {reason}",
                    g.format.as_str(),
                    target.as_str()
                ),
            ));
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
        return Err(CliError::usage("noTool", "run needs a tool name and files"));
    };
    if o.positional.len() < 2 {
        return Err(CliError::usage("noFiles", "run needs at least one file"));
    }
    if o.skip_same {
        return Err(CliError::usage(
            "optionNotForCommand",
            "--skip-same is for convert; a tool has no target format to compare",
        ));
    }
    let user = build_params(o.params.as_deref(), &o.sets)?;
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
            CliError::usage(
                "unknownTool",
                format!("unknown tool {tool:?}; tools: {}", names.join(", ")),
            )
        })?;
    let mut user = user;
    // A tool that sets a password (pdf-protect) takes it from the same
    // places as the password for an encrypted input.
    if let Some(pw) = &s.password {
        let unlocking = user.get("mode").and_then(Value::as_str) == Some("unlock");
        if !unlocking && entry.params_schema["properties"]["userPassword"].is_object() {
            user.entry("userPassword")
                .or_insert_with(|| Value::String(pw.expose().to_owned()));
        }
    }
    let params = with_defaults(&entry.params_schema, Value::Object(user));

    let mut notes = Notes::default();
    let loaded = load(&s, &files_only, &mut notes)?;
    let files = actionable(o, loaded, &mut notes)?;
    check_interrupt()?;
    let ids: Vec<String> = files.iter().map(|f| f.id.clone()).collect();
    let job = s.app.create_job(&tool, &ids, params).map_err(cmd_err)?;
    let dest = match &o.out {
        Some(p) => Dest::Auto(p.clone()),
        None => Dest::Folder(PathBuf::from(".")),
    };
    finish(&s, o, &job, dest, notes)
}

// ------------------------------------------------------------- job and save

enum Dest {
    /// A folder, keeping the layout of given folders with `--keep-folders`.
    Folder(PathBuf),
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

/// Waits for the job to end. On Ctrl-C it cancels the job once and keeps
/// waiting, a little while, for the runner to stop (audit R17).
fn wait_terminal(app: &App, job: &JobId) -> Res<JobSnapshot> {
    let mut cancelled_at: Option<Instant> = None;
    loop {
        if cancelled_at.is_none() && signal::interrupted() {
            let _ = app.cancel_job(job);
            cancelled_at = Some(Instant::now());
        }
        let snap = app
            .get_snapshot()
            .jobs
            .into_iter()
            .find(|j| &j.job_id == job)
            .ok_or_else(|| {
                CliError::failed("E_UNKNOWN_JOB", "unknownJob", "the job disappeared")
            })?;
        if snap.state.is_terminal() {
            return Ok(snap);
        }
        // Shutdown (when the session drops) cancels and joins what is left.
        if cancelled_at.is_some_and(|t| t.elapsed() > App::SHUTDOWN_WAIT) {
            return Err(CliError::cancelled());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn warnings_json(j: &JobSnapshot) -> Vec<Value> {
    j.warnings
        .iter()
        .map(|w| json!({ "code": w.code, "detail": w.detail, "message": words::warning(&w.code) }))
        .collect()
}

fn finish(s: &Session, o: &Opts, job: &JobId, dest: Dest, notes: Notes) -> Res<()> {
    let snap = match wait_terminal(&s.app, job) {
        Ok(snap) => snap,
        Err(e) => {
            let _ = s.app.discard_job(job);
            return Err(e);
        }
    };
    // Cancelled by Ctrl-C: whatever the job produced is not saved.
    if signal::interrupted() || snap.state == JobState::Cancelled {
        let _ = s.app.discard_job(job);
        return Err(CliError::cancelled());
    }
    for w in &snap.warnings {
        let text = words::warning(&w.code);
        match &w.detail {
            Some(d) => eprintln!("warning: {text} ({}: {d})", w.code),
            None => eprintln!("warning: {text} ({})", w.code),
        }
    }
    let state = name_of(&snap.state);
    let usable =
        matches!(snap.state, JobState::Succeeded | JobState::Partial) && !snap.outputs.is_empty();
    if !usable {
        let _ = s.app.discard_job(job);
        let (code, retryable) = match &snap.error {
            Some(e) => (e.code.clone(), e.retryable),
            None => ("E_INTERNAL".into(), false),
        };
        let reason = if snap.error.is_some() {
            camel_of_code(&code)
        } else {
            "noOutputs".into()
        };
        let message = format!("the job failed: {}", words::message(&code, None));
        let report = json!({
            "ok": false, "state": state,
            "error": error_json(&code, &reason, &message), "retryable": retryable,
            "warnings": warnings_json(&snap), "skipped": notes.skipped,
            "folderTruncated": notes.folder_truncated,
        });
        return Err(CliError::failed(&code, &reason, message).with_json(o, report));
    }

    let (dest, out_dir) = match resolve(o, s, job, dest, snap.outputs.len()) {
        Ok(d) => d,
        Err(e) => {
            let _ = s.app.discard_job(job);
            return Err(e);
        }
    };
    let saved = match s.app.save_outputs(job, &dest, false) {
        Ok(r) => r,
        Err(e) => {
            let err = if e.code == "E_CONFIRM_OVERWRITE" {
                CliError::failed(
                    "E_CONFIRM_OVERWRITE",
                    "inputOverwrite",
                    "refusing to overwrite an input file (the app asks first; the command line never replaces one)",
                )
            } else {
                cmd_err(e)
            };
            let _ = s.app.discard_job(job);
            return Err(err);
        }
    };
    if saved.sync_provider.is_some() {
        eprintln!("note: the destination is in a cloud-synced folder");
    }
    if saved.cleanup_deferred && !o.quiet {
        eprintln!("note: the job's temporary folder couldn't be deleted yet; it is removed at the next start");
    }
    let shown: Vec<String> = saved
        .saved
        .iter()
        .map(|p| p.display().to_string())
        .collect();

    let partial = snap.state == JobState::Partial;
    if o.json {
        let mut v = json!({
            "ok": !partial, "state": state, "savedCount": shown.len(),
            "saved": shown, "outDir": out_dir.as_ref().map(|d| d.display().to_string()),
            "warnings": warnings_json(&snap), "skipped": notes.skipped,
            "skippedSameFormat": notes.skipped_same,
            "folderTruncated": notes.folder_truncated,
        });
        if partial {
            let message = words::warning("PART_FAILED");
            v["error"] = error_json("E_PARTIAL", "partFailed", message);
        }
        println!("{v}");
    } else {
        for p in &shown {
            println!("{p}");
        }
        if !o.quiet {
            match &out_dir {
                Some(d) => eprintln!("saved {} file(s) to {}", shown.len(), d.display()),
                None => eprintln!("saved {} file(s)", shown.len()),
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

/// The save destination, and the folder it is in (for the closing line).
fn resolve(
    o: &Opts,
    s: &Session,
    job: &JobId,
    dest: Dest,
    outputs: usize,
) -> Res<(SaveDestination, Option<PathBuf>)> {
    let make = |dir: &PathBuf| {
        std::fs::create_dir_all(dir).map_err(|e| {
            CliError::failed(
                "E_SAVE_FAILED",
                "cantCreateFolder",
                format!("{}: {e}", dir.display()),
            )
        })
    };
    match dest {
        Dest::Folder(dir) => {
            make(&dir)?;
            let d = s.app.folder_destination(job, dir.clone(), o.keep_folders);
            Ok((d, Some(dir)))
        }
        Dest::Auto(path) => {
            // Asking to keep folders means --out is a folder, even for one result.
            let keeping = o.keep_folders && s.app.can_keep_layout(job);
            let as_file = !keeping
                && outputs == 1
                && !path.is_dir()
                && !path.to_string_lossy().ends_with(['/', '\\']);
            if !as_file {
                make(&path)?;
                let d = s.app.folder_destination(job, path.clone(), o.keep_folders);
                return Ok((d, Some(path)));
            }
            if path.exists() && !o.force {
                return Err(CliError::usage(
                    "outExists",
                    format!(
                        "{} exists; pass --force to replace it, or give a folder",
                        path.display()
                    ),
                ));
            }
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                make(&parent.to_path_buf())?;
            }
            Ok((SaveDestination::File(path), None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use converter_app_core::Reason;

    #[test]
    fn any_password_failure_is_reported_with_its_reason() {
        let wrong = CommandError::with("E_PASSWORD_WRONG", Reason::UnknownFile);
        assert_eq!(password_failure(&wrong).0, "passwordWrong");
        // Before, only a wrong password was reported; the rest vanished.
        let other = CommandError::with("E_INTERNAL", Reason::UnknownFile);
        let (reason, message) = password_failure(&other);
        assert_ne!(reason, "passwordWrong");
        assert!(!reason.is_empty() && !message.is_empty());
        let office = CommandError::with("E_PASSWORD_REQUIRED", Reason::OfficePassword);
        assert_eq!(password_failure(&office).0, "officePassword");
    }

    #[test]
    fn skip_same_rides_in_the_params_only_when_asked() {
        let p = json!({"quality": 80});
        assert_eq!(with_skip_same(p.clone(), false), p);
        assert_eq!(
            with_skip_same(p, true),
            json!({"quality": 80, "skipSame": true})
        );
    }

    #[test]
    fn first_line_strips_the_line_end_and_rejects_empty() {
        let pw = |s: &str| first_line(s.as_bytes()).map(|p| p.expose().to_owned());
        assert_eq!(pw("hunter2\r\nmore\n").as_deref(), Some("hunter2"));
        assert_eq!(pw("no-newline").as_deref(), Some("no-newline"));
        assert_eq!(pw("\nsecond\n"), None);
        assert_eq!(pw(""), None);
    }
}
