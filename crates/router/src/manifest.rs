//! Adapter manifests (spec section 8.2) and the generated registry.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Format, Platform, Tier};

/// `adapters/manifest.schema.json`, embedded so the schema and loader can't drift.
pub const MANIFEST_SCHEMA_JSON: &str = include_str!("../../../adapters/manifest.schema.json");

/// Custom form widgets (spec section 12.3). `x-ui` values in a parameter
/// schema must be one of these.
pub const WIDGETS: &[&str] = &[
    "pageRanges",
    "pageOrganizer",
    "redactionCanvas",
    "password",
    "fileOrder",
    "imagePicker",
];

pub const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Operation,
    Conversion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerClass {
    Pdf,
    Image,
    Render,
    Office,
    Ocr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cardinality {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Inputs {
    pub formats: Vec<Format>,
    pub min: u32,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Outputs {
    pub format: Format,
    pub cardinality: Cardinality,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeDecl {
    pub from: Vec<Format>,
    pub to: Format,
    pub tier: Tier,
    #[serde(default = "default_cost")]
    pub cost: u32,
}

fn default_cost() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub timeout_base_s: u32,
    pub timeout_s_per_mb: f64,
    pub no_progress_s: u32,
    pub disk_factor: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    #[serde(default)]
    pub dev_only: bool,
    pub worker_class: WorkerClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Inputs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Outputs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<EdgeDecl>>,
    #[serde(default)]
    pub fidelity_notes: Vec<String>,
    pub limits: Limits,
    pub params: String,
    pub platforms: Vec<Platform>,
    pub requires: Vec<String>,
}

/// One registry entry: the manifest plus its parameter schema, inlined so
/// the UI can generate forms without reading adapter folders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterEntry {
    pub manifest: Manifest,
    pub params_schema: Value,
}

/// `registry.generated.json` (gitignored; spec section 8.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    pub version: u32,
    /// Sorted by ID, so output is deterministic.
    pub adapters: Vec<AdapterEntry>,
}

impl Registry {
    pub fn get(&self, id: &str) -> Option<&AdapterEntry> {
        self.adapters.iter().find(|a| a.manifest.id == id)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("registry serializes");
        s.push('\n');
        s
    }
}

/// One thing wrong with the manifests. Generation reports all of them at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Adapter folder name, or `None` for repository-level problems.
    pub adapter: Option<String>,
    pub message: String,
}

impl Problem {
    fn new(adapter: Option<&str>, message: impl Into<String>) -> Self {
        Problem {
            adapter: adapter.map(str::to_owned),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.adapter {
            Some(a) => write!(f, "adapters/{a}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// What manifests are checked against: the warning catalog and approved engines.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub warnings: BTreeSet<String>,
    pub engines: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnginesFile {
    engines: BTreeMap<String, EngineEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // Fields are checked for shape; packaging (T21) reads them.
struct EngineEntry {
    license: String,
    version: Option<String>,
}

impl Catalog {
    /// Warning codes from the protocol schema (`#/definitions/WarningCode`)
    /// and engines from `engines/engines.yaml` under `root`.
    pub fn load(root: &Path) -> Result<Catalog, Vec<Problem>> {
        let mut problems = Vec::new();
        let warnings = warning_catalog();
        let path = root.join("engines").join("engines.yaml");
        let engines = match fs::read_to_string(&path) {
            Err(e) => {
                problems.push(Problem::new(None, format!("engines/engines.yaml: {e}")));
                BTreeSet::new()
            }
            Ok(text) => match serde_norway::from_str::<EnginesFile>(&text) {
                Ok(f) => f.engines.into_keys().collect(),
                Err(e) => {
                    problems.push(Problem::new(None, format!("engines/engines.yaml: {e}")));
                    BTreeSet::new()
                }
            },
        };
        if problems.is_empty() {
            Ok(Catalog { warnings, engines })
        } else {
            Err(problems)
        }
    }
}

fn warning_catalog() -> BTreeSet<String> {
    let schema: Value =
        serde_json::from_str(converter_protocol::SCHEMA_JSON).expect("protocol schema is JSON");
    schema["definitions"]["WarningCode"]["enum"]
        .as_array()
        .expect("protocol schema defines WarningCode as an enum")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

/// Read every `adapters/<id>/manifest.yaml` under `adapters_dir` and build
/// the registry, or return every problem found.
pub fn build_registry(
    adapters_dir: &Path,
    catalog: &Catalog,
    include_dev: bool,
) -> Result<Registry, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut adapters = Vec::new();

    let schema: Value =
        serde_json::from_str(MANIFEST_SCHEMA_JSON).expect("manifest schema is JSON");
    let validator = jsonschema::validator_for(&schema).expect("manifest schema compiles");

    let mut folders: Vec<_> = match fs::read_dir(adapters_dir) {
        // No adapters yet is a valid, empty registry.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(vec![Problem::new(
                None,
                format!("{}: {e}", adapters_dir.display()),
            )])
        }
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.') && name != "__pycache__")
            .collect(),
    };
    folders.sort();

    for folder in &folders {
        let before = problems.len();
        if let Some(entry) = load_adapter(adapters_dir, folder, &validator, catalog, &mut problems)
        {
            if problems.len() == before && (include_dev || !entry.manifest.dev_only) {
                adapters.push(entry);
            }
        }
    }

    let mut seen = BTreeSet::new();
    for a in &adapters {
        if !seen.insert(a.manifest.id.clone()) {
            problems.push(Problem::new(
                None,
                format!("duplicate adapter id {:?}", a.manifest.id),
            ));
        }
    }

    if problems.is_empty() {
        adapters.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(Registry {
            version: REGISTRY_VERSION,
            adapters,
        })
    } else {
        Err(problems)
    }
}

fn load_adapter(
    adapters_dir: &Path,
    folder: &str,
    validator: &jsonschema::Validator,
    catalog: &Catalog,
    problems: &mut Vec<Problem>,
) -> Option<AdapterEntry> {
    let dir = adapters_dir.join(folder);
    let mut problem = |m: String| problems.push(Problem::new(Some(folder), m));

    let text = match fs::read_to_string(dir.join("manifest.yaml")) {
        Ok(t) => t,
        Err(e) => {
            problem(format!("manifest.yaml: {e}"));
            return None;
        }
    };
    let value: Value = match serde_norway::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            problem(format!("manifest.yaml is not valid YAML: {e}"));
            return None;
        }
    };
    let schema_errors: Vec<String> = validator
        .iter_errors(&value)
        .map(|e| {
            format!(
                "manifest.yaml{}: {e}",
                pointer_suffix(&e.instance_path().to_string())
            )
        })
        .collect();
    if !schema_errors.is_empty() {
        schema_errors.into_iter().for_each(&mut problem);
        return None;
    }
    let manifest: Manifest = match serde_json::from_value(value) {
        Ok(m) => m,
        Err(e) => {
            problem(format!("manifest.yaml: {e}"));
            return None;
        }
    };

    if manifest.id != folder {
        problem(format!("id {:?} must equal the folder name", manifest.id));
    }
    if let Some(inputs) = &manifest.inputs {
        if inputs.min > inputs.max {
            problem(format!(
                "inputs.min ({}) exceeds inputs.max ({})",
                inputs.min, inputs.max
            ));
        }
    }
    if let Some(edges) = &manifest.edges {
        let mut pairs = BTreeSet::new();
        for edge in edges {
            for &from in &edge.from {
                if from == edge.to {
                    problem(format!(
                        "edge {} -> {} converts a format to itself",
                        from.as_str(),
                        edge.to.as_str()
                    ));
                }
                if !pairs.insert((from, edge.to)) {
                    problem(format!(
                        "edge {} -> {} is declared twice",
                        from.as_str(),
                        edge.to.as_str()
                    ));
                }
            }
        }
    }
    for code in &manifest.fidelity_notes {
        if !catalog.warnings.contains(code) {
            problem(format!(
                "fidelityNotes: {code} is not in the warning catalog (spec section 13.1)"
            ));
        }
    }
    for engine in &manifest.requires {
        if !catalog.engines.contains(engine) {
            problem(format!(
                "requires: engine {engine:?} is not approved in engines/engines.yaml"
            ));
        }
    }

    let params_schema = load_params_schema(&dir, &manifest.params, &mut problem)?;
    Some(AdapterEntry {
        manifest,
        params_schema,
    })
}

fn pointer_suffix(pointer: &str) -> String {
    if pointer.is_empty() {
        String::new()
    } else {
        format!(" at {pointer}")
    }
}

fn load_params_schema(dir: &Path, params: &str, problem: &mut impl FnMut(String)) -> Option<Value> {
    let name = params.trim_start_matches("./");
    let text = match fs::read_to_string(dir.join(name)) {
        Ok(t) => t,
        Err(e) => {
            problem(format!("{name}: {e}"));
            return None;
        }
    };
    let schema: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            problem(format!("{name} is not valid JSON: {e}"));
            return None;
        }
    };
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        problem(format!("{name}: the root must have \"type\": \"object\""));
    }
    if let Err(e) = jsonschema::meta::validate(&schema) {
        problem(format!("{name} is not a valid JSON Schema: {e}"));
        return None;
    }
    if let Err(e) = jsonschema::validator_for(&schema) {
        problem(format!("{name} does not compile: {e}"));
        return None;
    }
    let mut widgets = Vec::new();
    collect_widgets(&schema, &mut widgets);
    for w in widgets {
        if !WIDGETS.contains(&w.as_str()) {
            problem(format!(
                "{name}: unknown x-ui widget {w:?} (spec section 12.3)"
            ));
        }
    }
    Some(schema)
}

fn collect_widgets(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == "x-ui" {
                    out.push(
                        child
                            .as_str()
                            .map_or_else(|| child.to_string(), str::to_owned),
                    );
                } else {
                    collect_widgets(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|c| collect_widgets(c, out)),
        _ => {}
    }
}
