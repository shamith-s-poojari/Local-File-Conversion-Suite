//! Hand-rolled argument parsing (no clap in the workspace).

use std::path::PathBuf;

use serde_json::{Map, Value};

/// A command-line mistake: `reason` is a stable camelCase name for `--json`,
/// `message` is for people.
#[derive(Debug, PartialEq)]
pub struct UsageError {
    pub reason: &'static str,
    pub message: String,
}

pub fn usage(reason: &'static str, message: impl Into<String>) -> UsageError {
    UsageError {
        reason,
        message: message.into(),
    }
}

const PASSWORD_ADVICE: &str = "a password on the command line shows up in `ps` and shell history: use --password-stdin (one line on standard input) or the LFCS_PASSWORD environment variable";

#[derive(Debug, Default, PartialEq)]
pub struct Opts {
    pub command: Option<String>,
    pub positional: Vec<String>,
    pub json: bool,
    pub quiet: bool,
    pub force: bool,
    pub keep_folders: bool,
    pub help: bool,
    /// Use a repository checkout as the runtime (also `LFCS_DEV=1`).
    pub dev: bool,
    /// Read the password for encrypted inputs, and for tools that set one,
    /// from the first line of standard input.
    pub password_stdin: bool,
    pub version: bool,
    pub resources: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub to: Option<String>,
    pub out: Option<PathBuf>,
    pub sets: Vec<String>,
    pub params: Option<String>,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Opts, UsageError> {
    let mut o = Opts::default();
    let mut it = args.into_iter();
    let mut only_positional = false;
    while let Some(arg) = it.next() {
        if only_positional || !arg.starts_with('-') || arg == "-" {
            push(&mut o, arg);
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, UsageError> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| usage("missingValue", format!("{name} needs a value")))
        };
        match flag.as_str() {
            "-h" | "--help" => o.help = true,
            "--version" | "-V" => o.version = true,
            "--json" => o.json = true,
            "-q" | "--quiet" => o.quiet = true,
            "--force" => o.force = true,
            "--keep-folders" => o.keep_folders = true,
            "--dev" => o.dev = true,
            "--password-stdin" => o.password_stdin = true,
            // Refused rather than ignored: the value would already be in
            // argv by the time we look at it. The value is never echoed.
            "--password" => return Err(usage("passwordInArgv", PASSWORD_ADVICE)),
            "--resources" => o.resources = Some(value("--resources")?.into()),
            "--data-dir" => o.data_dir = Some(value("--data-dir")?.into()),
            "--to" => o.to = Some(value("--to")?),
            "-o" | "--out" => o.out = Some(value("--out")?.into()),
            "--set" => {
                let v = value("--set")?;
                o.sets.push(v);
            }
            "--params" => o.params = Some(value("--params")?),
            _ => return Err(usage("unknownOption", format!("unknown option {arg:?}"))),
        }
        if inline.is_some()
            && matches!(
                flag.as_str(),
                "--json"
                    | "-q"
                    | "--quiet"
                    | "--force"
                    | "--keep-folders"
                    | "--dev"
                    | "--password-stdin"
                    | "--help"
                    | "--version"
            )
        {
            return Err(usage("flagTakesNoValue", format!("{flag} takes no value")));
        }
    }
    Ok(o)
}

fn push(o: &mut Opts, arg: String) {
    if o.command.is_none() {
        o.command = Some(arg);
    } else {
        o.positional.push(arg);
    }
}

/// `--set key=value`: the value is JSON when it parses (numbers, booleans,
/// arrays, quoted strings), else a plain string.
pub fn parse_set(s: &str) -> Result<(String, Value), UsageError> {
    let (k, v) = s
        .split_once('=')
        .filter(|(k, _)| !k.is_empty())
        .ok_or_else(|| {
            // Not echoed: a malformed `--set password` could be the secret.
            usage("badSet", "--set wants key=value")
        })?;
    refuse_password_key(k)?;
    let value = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
    Ok((k.to_string(), value))
}

/// Passwords travel on standard input or in the environment, never as a
/// parameter (spec 7.3: they aren't params in the app either).
fn refuse_password_key(key: &str) -> Result<(), UsageError> {
    if key.to_ascii_lowercase().contains("password") {
        Err(usage("passwordInArgv", PASSWORD_ADVICE))
    } else {
        Ok(())
    }
}

/// `--params` JSON, then each `--set` over it.
pub fn build_params(
    params: Option<&str>,
    sets: &[String],
) -> Result<Map<String, Value>, UsageError> {
    let mut map = match params {
        None => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err(usage("badParams", "--params must be a JSON object")),
            // The parser's message can quote the text, which may hold a secret.
            Err(_) => return Err(usage("badParams", "--params isn't valid JSON")),
        },
    };
    for k in map.keys() {
        refuse_password_key(k)?;
    }
    for s in sets {
        let (k, v) = parse_set(s)?;
        map.insert(k, v);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Opts, UsageError> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags_anywhere() {
        let o = p(&[
            "convert",
            "a.png",
            "--to",
            "jpg",
            "--json",
            "b.png",
            "--set=quality=80",
        ])
        .unwrap();
        assert_eq!(o.command.as_deref(), Some("convert"));
        assert_eq!(o.positional, ["a.png", "b.png"]);
        assert_eq!(o.to.as_deref(), Some("jpg"));
        assert!(o.json);
        assert_eq!(o.sets, ["quality=80"]);
    }

    #[test]
    fn errors() {
        assert!(p(&["x", "--nope"]).is_err());
        assert!(p(&["x", "--to"]).is_err());
        assert!(p(&["x", "--json=1"]).is_err());
    }

    #[test]
    fn passwords_never_come_from_argv() {
        for bad in [
            &["run", "pdf-protect", "--password", "hunter2"][..],
            &["run", "pdf-protect", "--password=hunter2"][..],
        ] {
            let e = p(bad).unwrap_err();
            assert_eq!(e.reason, "passwordInArgv");
            assert!(!e.message.contains("hunter2"));
            assert!(e.message.contains("--password-stdin"));
            assert!(e.message.contains("LFCS_PASSWORD"));
        }
        for set in ["userPassword=hunter2", "password=x", "ownerPassword=x"] {
            let e = parse_set(set).unwrap_err();
            assert_eq!(e.reason, "passwordInArgv", "{set}");
            assert!(!e.message.contains("hunter2"));
        }
        let e = build_params(Some(r#"{"userPassword":"hunter2"}"#), &[]).unwrap_err();
        assert_eq!(e.reason, "passwordInArgv");
        assert!(!e.message.contains("hunter2"));
        let o = p(&["run", "pdf-protect", "--password-stdin", "--dev"]).unwrap();
        assert!(o.password_stdin && o.dev);
        assert!(p(&["x", "--password-stdin=1"]).is_err());
    }

    #[test]
    fn dashes_end_options() {
        let o = p(&["run", "t", "--", "-weird.png"]).unwrap();
        assert_eq!(o.positional, ["t", "-weird.png"]);
    }

    #[test]
    fn set_values() {
        assert_eq!(parse_set("q=80").unwrap().1, 80);
        assert_eq!(parse_set("b=true").unwrap().1, true);
        assert_eq!(
            parse_set("order=[1,2]").unwrap().1,
            serde_json::json!([1, 2])
        );
        assert_eq!(parse_set("bg=#fff").unwrap().1, "#fff");
        assert_eq!(
            parse_set("name=a=b").unwrap(),
            ("name".into(), "a=b".into())
        );
        assert!(parse_set("novalue").is_err());
        assert!(parse_set("=x").is_err());
    }

    #[test]
    fn set_overrides_params() {
        let m = build_params(Some(r#"{"a":1,"b":2}"#), &["b=3".into()]).unwrap();
        assert_eq!(m["a"], 1);
        assert_eq!(m["b"], 3);
        assert!(build_params(Some("[1]"), &[]).is_err());
        assert!(build_params(Some("{"), &[]).is_err());
    }
}
