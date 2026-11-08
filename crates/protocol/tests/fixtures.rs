//! Checks the codec against the shared fixtures in packages/protocol/fixtures.

use converter_protocol::types::{CoreMessage, WorkerMessage};
use converter_protocol::{
    Codec, DecodeError, Direction, SecretString, Secrets, MAX_LINE_BYTES, PROTOCOL_VERSION,
    SCHEMA_JSON,
};
use serde_json::{json, Value};

const CASES: &str = include_str!("../../../packages/protocol/fixtures/cases.json");

fn cases(kind: &str) -> Vec<(String, Direction, Value)> {
    let all: Value = serde_json::from_str(CASES).unwrap();
    all[kind]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            let direction = match case["direction"].as_str().unwrap() {
                "core" => Direction::Core,
                "worker" => Direction::Worker,
                other => panic!("unknown direction {other}"),
            };
            (
                case["name"].as_str().unwrap().to_owned(),
                direction,
                case["message"].clone(),
            )
        })
        .collect()
}

fn line(message: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(message).unwrap();
    bytes.push(b'\n');
    bytes
}

/// Makes `72` and `72.0` compare equal: `number` fields decode as f64.
fn numbers_as_f64(value: Value) -> Value {
    match value {
        Value::Number(n) => json!(n.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.into_iter().map(numbers_as_f64).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, numbers_as_f64(v)))
                .collect(),
        ),
        other => other,
    }
}

/// Exhaustive on purpose: adding a message to the schema breaks this match
/// until the new variant is mapped, and the fixtures then prove the mapping.
fn core_type(message: &CoreMessage) -> &'static str {
    match message {
        CoreMessage::Inspect(_) => "inspect",
        CoreMessage::Validate(_) => "validate",
        CoreMessage::Estimate(_) => "estimate",
        CoreMessage::Execute(_) => "execute",
        CoreMessage::Render(_) => "render",
        CoreMessage::PageText(_) => "pageText",
        CoreMessage::Cancel(_) => "cancel",
    }
}

fn worker_type(message: &WorkerMessage) -> &'static str {
    match message {
        WorkerMessage::Hello(_) => "hello",
        WorkerMessage::Stage(_) => "stage",
        WorkerMessage::Progress(_) => "progress",
        WorkerMessage::WorkerWarning(_) => "warning",
        WorkerMessage::WorkerError(_) => "error",
        WorkerMessage::InspectResult(_) => "inspectResult",
        WorkerMessage::ValidateResult(_) => "validateResult",
        WorkerMessage::EstimateResult(_) => "estimateResult",
        WorkerMessage::ExecuteResult(_) => "executeResult",
        WorkerMessage::RenderResult(_) => "renderResult",
        WorkerMessage::PageTextResult(_) => "pageTextResult",
    }
}

#[test]
fn accepts_valid_messages_as_the_right_variant() {
    let codec = Codec::new();
    for (name, direction, message) in cases("valid") {
        let expected = message["type"].as_str().unwrap();
        let (actual, encoded) = match direction {
            Direction::Core => {
                let decoded = codec
                    .decode_core(&line(&message))
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                (core_type(&decoded), Codec::encode(&decoded))
            }
            Direction::Worker => {
                let decoded = codec
                    .decode_worker(&line(&message))
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                (worker_type(&decoded), Codec::encode(&decoded))
            }
        };
        assert_eq!(actual, expected, "{name}: decoded as the wrong variant");
        let round_trip: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            numbers_as_f64(round_trip),
            numbers_as_f64(message),
            "{name}: round trip changed the message"
        );
    }
}

#[test]
fn rejects_invalid_messages() {
    let codec = Codec::new();
    for (name, direction, message) in cases("invalid") {
        let result = codec.validate(direction, &line(&message));
        assert!(
            matches!(result, Err(DecodeError::Schema { .. })),
            "{name}: expected a schema error, got {result:?}"
        );
    }
}

#[test]
fn rejects_long_lines_and_bad_json() {
    let codec = Codec::new();
    let long = vec![b' '; MAX_LINE_BYTES + 1];
    assert_eq!(
        codec.validate(Direction::Worker, &long),
        Err(DecodeError::TooLong {
            len: MAX_LINE_BYTES + 1
        })
    );
    assert!(matches!(
        codec.validate(Direction::Worker, b"{\"v\":1,"),
        Err(DecodeError::InvalidJson { .. })
    ));
    assert!(matches!(
        codec.validate(Direction::Worker, &[0xff, 0xfe]),
        Err(DecodeError::InvalidJson { .. })
    ));
}

#[test]
fn constants_match_schema() {
    let schema: Value = serde_json::from_str(SCHEMA_JSON).unwrap();
    assert_eq!(
        schema["definitions"]["ProtocolVersion"]["const"],
        json!(PROTOCOL_VERSION)
    );
}

#[test]
fn hand_written_secrets_match_schema() {
    let schema: Value = serde_json::from_str(SCHEMA_JSON).unwrap();
    let mut in_schema: Vec<_> = schema["definitions"]["Secrets"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    in_schema.sort();

    let secret = || Some(SecretString::new("x".into()));
    let all = Secrets {
        password: secret(),
        user_password: secret(),
        owner_password: secret(),
    };
    let encoded = serde_json::to_value(&all).unwrap();
    let mut in_rust: Vec<_> = encoded.as_object().unwrap().keys().cloned().collect();
    in_rust.sort();
    assert_eq!(in_rust, in_schema);
}

#[test]
fn secrets_never_appear_in_debug_or_errors() {
    let sentinel = "sentinel-PASSWORD-1";
    let codec = Codec::new();
    let valid = json!({"v": 1, "type": "inspect", "jobId": "j", "input": "in/0.pdf",
                       "secrets": {"password": sentinel}});
    let decoded = codec.decode_core(&line(&valid)).unwrap();
    assert!(!format!("{decoded:?}").contains(sentinel));
    assert!(String::from_utf8(Codec::encode(&decoded))
        .unwrap()
        .contains(sentinel));

    let invalid = json!({"v": 1, "type": "inspect", "jobId": "j", "input": "../escape",
                         "secrets": {"password": sentinel}});
    let error = codec
        .validate(Direction::Core, &line(&invalid))
        .unwrap_err();
    assert!(!error.to_string().contains(sentinel));
    assert!(!format!("{error:?}").contains(sentinel));
}

#[test]
fn errors_do_not_echo_document_text() {
    let codec = Codec::new();
    let text = "TOP-SECRET-DOCUMENT-TEXT";
    let message = json!({"v": 1, "type": "pageTextResult", "jobId": "j", "page": 1,
                         "width": 10, "height": 10,
                         "words": [{"text": text, "x0": 0, "y0": 0, "x1": 1, "y1": 1, "extra": 1}]});
    let error = codec
        .validate(Direction::Worker, &line(&message))
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains(text));
}
