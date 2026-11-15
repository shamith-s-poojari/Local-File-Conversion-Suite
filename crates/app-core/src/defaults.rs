//! Parameter defaults from an adapter's params schema, for callers with no
//! form (the `lfcs` command line). Mirrors `defaults` in the desktop app's
//! `SchemaForm.tsx`: each top-level property's `default`, except params the
//! core fills or a later user decision sets. A required enum with no default
//! (a conversion's `to`) takes its first value, so a required field is never
//! left out.

use serde_json::{Map, Value};

/// Never filled: set by the core, or only by a later user decision.
const HIDDEN: &[&str] = &["fileTitles", "rasterizeFallback"];

pub fn defaults(schema: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return out;
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (key, prop) in props {
        if HIDDEN.contains(&key.as_str()) {
            continue;
        }
        if let Some(d) = prop.get("default") {
            out.insert(key.clone(), d.clone());
        } else if required.contains(&key.as_str()) {
            if let Some(first) = prop
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|e| e.first())
            {
                out.insert(key.clone(), first.clone());
            }
        }
    }
    out
}

/// `params` over the schema defaults. Non-object params are returned as is.
pub fn with_defaults(schema: &Value, params: Value) -> Value {
    let Value::Object(given) = params else {
        return params;
    };
    let mut out = defaults(schema);
    out.extend(given);
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn schema(adapter: &str) -> Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../adapters")
            .join(adapter)
            .join("params.schema.json");
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn video_frames_has_a_to() {
        let d = defaults(&schema("video-frames"));
        assert_eq!(d["to"], "jpg");
        assert_eq!(d["mode"], "count");
        assert_eq!(d["count"], 5);
        assert!(!d.contains_key("fileTitles"));
    }

    #[test]
    fn pdf_to_image_required_enum_takes_first() {
        let d = defaults(&schema("pdf-to-image"));
        assert_eq!(d["to"], "png");
        assert_eq!(d["dpi"], 150);
        assert_eq!(d["multipage"], true);
        assert!(!d.contains_key("pages"));
    }

    #[test]
    fn given_params_win() {
        let v = with_defaults(&schema("pdf-to-image"), json!({"to": "jpg", "dpi": 300}));
        assert_eq!(v["to"], "jpg");
        assert_eq!(v["dpi"], 300);
        assert_eq!(v["quality"], 85);
    }

    #[test]
    fn real_schemas_validate_once_filled() {
        for a in ["video-frames", "pdf-to-image", "pdf-merge", "image-convert"] {
            let s = schema(a);
            let v = with_defaults(&s, json!({}));
            let validator = jsonschema::validator_for(&s).unwrap();
            assert!(validator.is_valid(&v), "{a}: {v}");
        }
    }

    #[test]
    fn odd_schemas() {
        assert!(defaults(&json!({})).is_empty());
        assert_eq!(with_defaults(&json!({}), json!(3)), json!(3));
    }
}
