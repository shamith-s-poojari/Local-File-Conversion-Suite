//! Registry generation: the fixture manifests load, and every rule in spec
//! section 8.2 fails the build with a useful message.

use std::fs;
use std::path::{Path, PathBuf};

use converter_router::{build_registry, Catalog, Problem, Registry};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/adapters")
}

fn catalog() -> Catalog {
    Catalog::load(&repo_root()).expect("repository catalog loads")
}

pub fn fixture_registry(include_dev: bool) -> Registry {
    build_registry(&fixtures(), &catalog(), include_dev).unwrap_or_else(|p| {
        panic!(
            "fixtures should load:\n{}",
            p.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

#[test]
fn fixtures_load() {
    let r = fixture_registry(true);
    assert_eq!(r.adapters.len(), 18);
    let ids: Vec<_> = r.adapters.iter().map(|a| a.manifest.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "registry is sorted by id");
}

#[test]
fn dev_only_adapters_are_left_out_by_default() {
    assert!(fixture_registry(false).get("dev-fake").is_none());
    assert!(fixture_registry(true).get("dev-fake").is_some());
}

#[test]
fn params_schema_is_inlined() {
    let r = fixture_registry(false);
    let merge = r.get("pdf-merge").unwrap();
    assert_eq!(
        merge.params_schema["properties"]["order"]["x-ui"],
        "fileOrder"
    );
}

#[test]
fn output_is_deterministic_json() {
    let a = fixture_registry(false).to_json();
    let b = fixture_registry(false).to_json();
    assert_eq!(a, b);
    let back: Registry = serde_json::from_str(&a).unwrap();
    assert_eq!(back, fixture_registry(false));
}

#[test]
fn the_real_adapters_folder_is_valid() {
    // Empty until the first adapter lands; must never be broken.
    build_registry(&repo_root().join("adapters"), &catalog(), true).unwrap_or_else(|p| {
        panic!(
            "{}",
            p.iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
    });
}

// ------------------------------------------------------------ failure cases

const GOOD_MANIFEST: &str = "\
id: sample
kind: operation
title: Sample
workerClass: pdf
inputs: { formats: [pdf], min: 1, max: 1 }
outputs: { format: pdf, cardinality: one }
fidelity: E
fidelityNotes: []
limits: { timeoutBaseS: 1, timeoutSPerMb: 1, noProgressS: 1, diskFactor: 1 }
params: ./params.schema.json
platforms: [linux]
requires: [pikepdf]
";

const GOOD_PARAMS: &str = r#"{"type": "object", "properties": {}}"#;

struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("router-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Sandbox { dir }
    }

    fn adapter(&self, folder: &str, manifest: &str, params: Option<&str>) -> &Self {
        let d = self.dir.join(folder);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("manifest.yaml"), manifest).unwrap();
        if let Some(p) = params {
            fs::write(d.join("params.schema.json"), p).unwrap();
        }
        self
    }

    fn problems(&self) -> Vec<Problem> {
        build_registry(&self.dir, &catalog(), true).expect_err("expected problems")
    }

    fn ok(&self) -> Registry {
        build_registry(&self.dir, &catalog(), true).expect("expected success")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn assert_problem(problems: &[Problem], needle: &str) {
    assert!(
        problems.iter().any(|p| p.to_string().contains(needle)),
        "expected a problem containing {needle:?}, got:\n{}",
        problems
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn good_manifest_passes() {
    let s = Sandbox::new("good");
    s.adapter("sample", GOOD_MANIFEST, Some(GOOD_PARAMS));
    assert_eq!(s.ok().adapters.len(), 1);
}

#[test]
fn missing_adapters_folder_is_an_empty_registry() {
    let r = build_registry(Path::new("does/not/exist"), &catalog(), false).unwrap();
    assert!(r.adapters.is_empty());
}

#[test]
fn id_must_match_folder() {
    let s = Sandbox::new("folder");
    s.adapter("other-name", GOOD_MANIFEST, Some(GOOD_PARAMS));
    assert_problem(
        &s.problems(),
        "adapters/other-name: id \"sample\" must equal the folder name",
    );
}

#[test]
fn unknown_warning_code_fails() {
    let s = Sandbox::new("warning");
    s.adapter(
        "sample",
        &GOOD_MANIFEST.replace("fidelityNotes: []", "fidelityNotes: [MADE_UP]"),
        Some(GOOD_PARAMS),
    );
    assert_problem(&s.problems(), "MADE_UP is not in the warning catalog");
}

#[test]
fn unapproved_engine_fails() {
    let s = Sandbox::new("engine");
    // ADR 0001: Calibre is explicitly not an approved dependency.
    s.adapter(
        "sample",
        &GOOD_MANIFEST.replace("[pikepdf]", "[calibre]"),
        Some(GOOD_PARAMS),
    );
    assert_problem(&s.problems(), "engine \"calibre\" is not approved");
}

#[test]
fn schema_violations_are_reported_with_location() {
    let s = Sandbox::new("schema");
    s.adapter(
        "sample",
        &GOOD_MANIFEST.replace("workerClass: pdf", "workerClass: gpu"),
        Some(GOOD_PARAMS),
    );
    assert_problem(&s.problems(), "manifest.yaml at /workerClass");
}

#[test]
fn unknown_fields_are_rejected() {
    let s = Sandbox::new("unknown");
    s.adapter(
        "sample",
        &format!("{GOOD_MANIFEST}network: true\n"),
        Some(GOOD_PARAMS),
    );
    assert_problem(&s.problems(), "network");
}

#[test]
fn operation_and_conversion_shapes_are_exclusive() {
    let s = Sandbox::new("shape");
    let with_edges = format!("{GOOD_MANIFEST}edges: [{{ from: [pdf], to: png, tier: H }}]\n");
    s.adapter("sample", &with_edges, Some(GOOD_PARAMS));
    assert!(!s.problems().is_empty());

    let s = Sandbox::new("shape2");
    let conversion_without_edges = GOOD_MANIFEST.replace("kind: operation", "kind: conversion");
    s.adapter("sample", &conversion_without_edges, Some(GOOD_PARAMS));
    assert!(!s.problems().is_empty());
}

#[test]
fn bad_edges_fail() {
    let s = Sandbox::new("edges");
    let manifest = "\
id: sample
kind: conversion
title: Sample
workerClass: image
edges:
  - { from: [png, jpg], to: png, tier: E }
  - { from: [jpg], to: png, tier: L }
limits: { timeoutBaseS: 1, timeoutSPerMb: 1, noProgressS: 1, diskFactor: 1 }
params: ./params.schema.json
platforms: [linux]
requires: [pillow]
";
    s.adapter("sample", manifest, Some(GOOD_PARAMS));
    let p = s.problems();
    assert_problem(&p, "edge png -> png converts a format to itself");
    assert_problem(&p, "edge jpg -> png is declared twice");
}

#[test]
fn min_above_max_fails() {
    let s = Sandbox::new("minmax");
    s.adapter(
        "sample",
        &GOOD_MANIFEST.replace("min: 1, max: 1", "min: 3, max: 2"),
        Some(GOOD_PARAMS),
    );
    assert_problem(&s.problems(), "inputs.min (3) exceeds inputs.max (2)");
}

#[test]
fn params_schema_problems() {
    let s = Sandbox::new("params-missing");
    s.adapter("sample", GOOD_MANIFEST, None);
    assert_problem(&s.problems(), "params.schema.json");

    let s = Sandbox::new("params-json");
    s.adapter("sample", GOOD_MANIFEST, Some("{ not json"));
    assert_problem(&s.problems(), "is not valid JSON");

    let s = Sandbox::new("params-type");
    s.adapter("sample", GOOD_MANIFEST, Some(r#"{"type": "array"}"#));
    assert_problem(&s.problems(), "the root must have \"type\": \"object\"");

    let s = Sandbox::new("params-invalid");
    s.adapter(
        "sample",
        GOOD_MANIFEST,
        Some(r#"{"type": "object", "minProperties": "three"}"#),
    );
    assert_problem(&s.problems(), "is not a valid JSON Schema");

    let s = Sandbox::new("params-widget");
    s.adapter(
        "sample",
        GOOD_MANIFEST,
        Some(
            r#"{"type": "object", "properties": {"a": {"type": "string", "x-ui": "colorWheel"}}}"#,
        ),
    );
    assert_problem(&s.problems(), "unknown x-ui widget \"colorWheel\"");
}

#[test]
fn params_path_cannot_leave_the_adapter_folder() {
    let s = Sandbox::new("params-escape");
    s.adapter(
        "sample",
        &GOOD_MANIFEST.replace("./params.schema.json", "../other/params.schema.json"),
        None,
    );
    assert_problem(&s.problems(), "manifest.yaml at /params");
}

#[test]
fn all_problems_are_reported_together() {
    let s = Sandbox::new("many");
    s.adapter(
        "a",
        &GOOD_MANIFEST.replace("[pikepdf]", "[calibre]"),
        Some(GOOD_PARAMS),
    );
    s.adapter("b", "not: [valid", None);
    let p = s.problems();
    assert!(p.iter().any(|p| p.adapter.as_deref() == Some("a")));
    assert!(p.iter().any(|p| p.adapter.as_deref() == Some("b")));
}
