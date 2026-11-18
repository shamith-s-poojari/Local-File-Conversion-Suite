//! The pure steps between a selection and a job: splitting secrets from
//! params, ordering and grouping files, collecting their inputs, and
//! flattening the parts a job runs. No locks and no I/O, so they're
//! unit-tested here; `jobs.rs` calls them in the right order.

use std::collections::{BTreeMap, BTreeSet};

use converter_jobstore::InputSource;
use converter_protocol::{SecretString, Secrets};
use converter_router::{matrix, Format as RouterFormat, Plan, Planner};
use converter_scheduler::JobPart;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::files::FileEntry;
use crate::{CommandError, CommandResult, Reason};

/// `pdf-protect` collects passwords in password widgets; they must travel
/// as secrets, never params (spec 7.3).
pub(super) fn split_secrets(params: Value) -> CommandResult<(Value, Option<Secrets>)> {
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
pub(super) fn order_files(params: &Value, file_ids: &[String]) -> CommandResult<Vec<String>> {
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
pub(super) fn group_by_format(
    ids: &[String],
    formats: &[RouterFormat],
) -> Vec<(RouterFormat, Vec<String>)> {
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
pub(super) type AdapterGroup = (String, Vec<String>, BTreeSet<RouterFormat>);

/// The selection split by the adapter that converts each format: (adapter,
/// its files, their formats), in the order files were given. `adapter_of` is
/// asked once per format.
pub(super) fn group_by_adapter(
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
pub(super) fn skip_same_format(
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
pub(super) fn formats_in_order(
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
pub(super) struct Collected {
    pub(super) sources: Vec<InputSource>,
    pub(super) folders: Vec<Option<String>>,
    pub(super) password: Option<SecretString>,
    pub(super) titles: Vec<String>,
}

/// Checks each file in `ids` (known, actionable, accepted) and gathers its
/// staged-input source, folder, title and the part's one password. Pure over
/// the selection map, so it runs under a short lock and is testable.
pub(super) fn collect_inputs(
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
pub(super) fn finish_params(params: &mut Value, wants_titles: bool, titles: &[String]) {
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
    pub(super) sources: Vec<InputSource>,
    /// The dropped folder of each source, in the same order.
    pub(super) folders: Vec<Option<String>>,
    /// The file IDs of the sources, in the same order.
    pub(super) file_ids: Vec<String>,
    pub(super) pool: String,
    pub(super) disk_factor: f64,
}

/// What [`App::start`] stages: every part's inputs end to end, in part order.
pub(super) struct Flattened {
    pub(super) sources: Vec<InputSource>,
    pub(super) folders: Vec<Option<String>>,
    pub(super) file_ids: Vec<String>,
    pub(super) parts: Vec<JobPart>,
}

/// Concatenates the parts' inputs. A part's `inputs` index this order, which
/// is grouped by adapter and so differs from the selection order.
pub(super) fn flatten(parts: Vec<Prepared>) -> Flattened {
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
pub(super) fn strictest_pool<'a>(pools: impl Iterator<Item = &'a str>) -> String {
    const ORDER: [&str; 7] = ["office", "media", "redact", "ocr", "render", "image", "pdf"];
    pools
        .min_by_key(|p| ORDER.iter().position(|o| o == p).unwrap_or(ORDER.len()))
        .unwrap_or("pdf")
        .to_owned()
}

pub(super) fn worker_class(m: &converter_router::Manifest) -> &'static str {
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
pub(super) fn best_plan(planner: &Planner, from: RouterFormat, to: RouterFormat) -> Option<Plan> {
    if from.in_blueprint() && to.in_blueprint() {
        matrix::offered(from, to).and_then(|cell| planner.best_for_cell(from, to, cell))
    } else {
        planner.best_direct(from, to)
    }
}

pub(super) fn stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_owned(),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use converter_sniffer::Format;

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
