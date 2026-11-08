//! Line codec: length limit, JSON parsing, schema validation, then typed decoding.

use serde::Serialize;
use serde_json::Value;

use crate::{CoreMessage, WorkerMessage, MAX_LINE_BYTES, SCHEMA_JSON};

/// Which side sent a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Core to worker.
    Core,
    /// Worker to core.
    Worker,
}

/// Why a line was rejected. For a worker line, every variant means `E_PROTOCOL`.
///
/// Errors never carry message content, which may include document text or
/// secrets. They carry only JSON pointer paths and schema locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The line is longer than [`MAX_LINE_BYTES`].
    TooLong { len: usize },
    /// The line is not valid UTF-8 JSON.
    InvalidJson { line: usize, column: usize },
    /// The message doesn't match the schema.
    Schema {
        instance_path: String,
        schema_path: String,
    },
    /// The message is schema-valid but the generated types couldn't hold it.
    /// This is a bug in this crate; the fixture tests are meant to rule it out.
    Unmapped { message_type: String },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong { len } => write!(f, "line of {len} bytes exceeds {MAX_LINE_BYTES}"),
            Self::InvalidJson { line, column } => {
                write!(f, "invalid JSON at line {line}, column {column}")
            }
            Self::Schema {
                instance_path,
                schema_path,
            } => write!(
                f,
                "schema violation at '{instance_path}' (rule '{schema_path}')"
            ),
            Self::Unmapped { message_type } => {
                write!(f, "no protocol type for valid '{message_type}' message")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Validates and decodes protocol lines. Build one and reuse it: compiling the
/// schema is the expensive part.
pub struct Codec {
    core: jsonschema::Validator,
    worker: jsonschema::Validator,
}

impl std::fmt::Debug for Codec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Codec { .. }")
    }
}

impl Default for Codec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec {
    /// Compiles the embedded schema.
    ///
    /// # Panics
    ///
    /// If the embedded schema is invalid, which the crate's tests rule out.
    pub fn new() -> Self {
        let root: Value = serde_json::from_str(SCHEMA_JSON).expect("embedded schema is JSON");
        Self {
            core: compile(&root, "CoreMessage"),
            worker: compile(&root, "WorkerMessage"),
        }
    }

    /// Decodes a line a worker sent to the core.
    pub fn decode_worker(&self, line: &[u8]) -> Result<WorkerMessage, DecodeError> {
        let value = self.validate(Direction::Worker, line)?;
        let message_type = message_type(&value);
        let typed = match message_type.as_str() {
            "hello" => typed(value).map(WorkerMessage::Hello),
            "stage" => typed(value).map(WorkerMessage::Stage),
            "progress" => typed(value).map(WorkerMessage::Progress),
            "warning" => typed(value).map(WorkerMessage::WorkerWarning),
            "error" => typed(value).map(WorkerMessage::WorkerError),
            "inspectResult" => typed(value).map(WorkerMessage::InspectResult),
            "validateResult" => typed(value).map(WorkerMessage::ValidateResult),
            "estimateResult" => typed(value).map(WorkerMessage::EstimateResult),
            "executeResult" => typed(value).map(WorkerMessage::ExecuteResult),
            "renderResult" => typed(value).map(WorkerMessage::RenderResult),
            "pageTextResult" => typed(value).map(WorkerMessage::PageTextResult),
            _ => None,
        };
        typed.ok_or(DecodeError::Unmapped { message_type })
    }

    /// Decodes a line the core sent to a worker. Used by tests and tools; the
    /// Python SDK is the real consumer.
    pub fn decode_core(&self, line: &[u8]) -> Result<CoreMessage, DecodeError> {
        let value = self.validate(Direction::Core, line)?;
        let message_type = message_type(&value);
        let typed = match message_type.as_str() {
            "inspect" => typed(value).map(CoreMessage::Inspect),
            "validate" => typed(value).map(CoreMessage::Validate),
            "estimate" => typed(value).map(CoreMessage::Estimate),
            "execute" => typed(value).map(CoreMessage::Execute),
            "render" => typed(value).map(CoreMessage::Render),
            "pageText" => typed(value).map(CoreMessage::PageText),
            "cancel" => typed(value).map(CoreMessage::Cancel),
            _ => None,
        };
        typed.ok_or(DecodeError::Unmapped { message_type })
    }

    /// Checks a line against the schema for `direction` and returns the parsed JSON.
    pub fn validate(&self, direction: Direction, line: &[u8]) -> Result<Value, DecodeError> {
        let line = line.strip_suffix(b"\n").unwrap_or(line);
        if line.len() > MAX_LINE_BYTES {
            return Err(DecodeError::TooLong { len: line.len() });
        }
        let value: Value = serde_json::from_slice(line).map_err(|e| DecodeError::InvalidJson {
            line: e.line(),
            column: e.column(),
        })?;
        let validator = match direction {
            Direction::Core => &self.core,
            Direction::Worker => &self.worker,
        };
        if let Some(error) = validator.iter_errors(&value).next() {
            return Err(DecodeError::Schema {
                instance_path: error.instance_path().to_string(),
                schema_path: error.schema_path().to_string(),
            });
        }
        Ok(value)
    }

    /// Encodes a message as one NDJSON line, including the trailing newline.
    ///
    /// # Panics
    ///
    /// If the encoded message is longer than [`MAX_LINE_BYTES`]; senders must
    /// keep messages small.
    pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
        let mut line = serde_json::to_vec(message).expect("protocol types always serialize");
        assert!(
            line.len() <= MAX_LINE_BYTES,
            "protocol message exceeds the line limit"
        );
        line.push(b'\n');
        line
    }
}

/// The `type` of a schema-valid message. The schema guarantees it is one of
/// the known constants, so it is safe to put in an error.
fn message_type(value: &Value) -> String {
    value["type"].as_str().unwrap_or_default().to_owned()
}

/// Deserializes a schema-valid message into its generated struct. The serde
/// error is dropped on purpose: its text can quote message content.
fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Option<T> {
    serde_json::from_value(value).ok()
}

/// Compiles the schema with `definition` as its root, keeping every definition
/// reachable for `$ref`.
fn compile(root: &Value, definition: &str) -> jsonschema::Validator {
    let mut schema = root.clone();
    let object = schema.as_object_mut().expect("schema is an object");
    object.remove("oneOf");
    object.remove("$id");
    object.insert(
        "$ref".into(),
        Value::String(format!("#/definitions/{definition}")),
    );
    jsonschema::draft7::options()
        .build(&schema)
        .expect("embedded schema compiles")
}
