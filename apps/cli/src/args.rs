//! Hand-rolled argument parsing (no clap in the workspace).

use std::path::PathBuf;

use serde_json::{Map, Value};

#[derive(Debug, Default, PartialEq)]
pub struct Opts {
    pub command: Option<String>,
    pub positional: Vec<String>,
    pub json: bool,
    pub quiet: bool,
    pub force: bool,
    pub keep_folders: bool,
    pub help: bool,
    pub version: bool,
    pub resources: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub to: Option<String>,
    pub out: Option<PathBuf>,
    pub sets: Vec<String>,
    pub params: Option<String>,
}

/// `Err` is a usage message.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Opts, String> {
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
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => o.help = true,
            "--version" | "-V" => o.version = true,
            "--json" => o.json = true,
            "-q" | "--quiet" => o.quiet = true,
            "--force" => o.force = true,
            "--keep-folders" => o.keep_folders = true,
            "--resources" => o.resources = Some(value("--resources")?.into()),
            "--data-dir" => o.data_dir = Some(value("--data-dir")?.into()),
            "--to" => o.to = Some(value("--to")?),
            "-o" | "--out" => o.out = Some(value("--out")?.into()),
            "--set" => {
                let v = value("--set")?;
                o.sets.push(v);
            }
            "--params" => o.params = Some(value("--params")?),
            _ => return Err(format!("unknown option {arg:?}")),
        }
        if inline.is_some()
            && matches!(
                flag.as_str(),
                "--json" | "-q" | "--quiet" | "--force" | "--keep-folders" | "--help" | "--version"
            )
        {
            return Err(format!("{flag} takes no value"));
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
pub fn parse_set(s: &str) -> Result<(String, Value), String> {
    let (k, v) = s
        .split_once('=')
        .filter(|(k, _)| !k.is_empty())
        .ok_or_else(|| format!("--set wants key=value, got {s:?}"))?;
    let value = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
    Ok((k.to_string(), value))
}

/// `--params` JSON, then each `--set` over it.
pub fn build_params(params: Option<&str>, sets: &[String]) -> Result<Map<String, Value>, String> {
    let mut map = match params {
        None => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err("--params must be a JSON object".into()),
            Err(e) => return Err(format!("--params isn't valid JSON: {e}")),
        },
    };
    for s in sets {
        let (k, v) = parse_set(s)?;
        map.insert(k, v);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Opts, String> {
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
