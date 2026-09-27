//! JSON structure and representative scalar values used by jq completion.

use serde_json::Value;

use super::api_spec::Schema;
use super::limits::SHAPE;

/// A bounded structural view shared by response bodies and API schemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonShape {
    Unknown,
    Object(Vec<(String, Self)>),
    Array {
        item: Box<Self>,
        length: Option<usize>,
    },
    Scalar {
        kind: &'static str,
        sample: Option<String>,
    },
}

impl JsonShape {
    /// Use the normalizer's bounded structure, without treating example values as fields.
    pub fn from_schema(schema: &Schema) -> Self {
        if schema.type_name == "array" {
            return Self::Array {
                item: Box::new(
                    schema
                        .items
                        .as_deref()
                        .map_or(Self::Unknown, Self::from_schema),
                ),
                length: None,
            };
        }
        if !schema.properties.is_empty() {
            return Self::Object(
                schema
                    .properties
                    .iter()
                    .map(|property| (property.name.clone(), Self::from_schema(&property.schema)))
                    .collect(),
            );
        }
        let kind = match schema.type_name.as_str() {
            "string" => "string",
            "integer" => "integer",
            "number" => "number",
            "boolean" => "boolean",
            "null" => "null",
            _ => return Self::Unknown,
        };
        Self::Scalar { kind, sample: None }
    }

    /// The shape of the `{status, headers, body}` envelope returned by
    /// `api --json` around a response body.
    pub fn response_envelope(body: Self) -> Self {
        Self::Object(vec![
            (
                "status".into(),
                Self::Scalar {
                    kind: "integer",
                    sample: None,
                },
            ),
            ("headers".into(), Self::Object(Vec::new())),
            ("body".into(), body),
        ])
    }

    pub fn from_value(value: &Value) -> Self {
        Self::value_at_depth(value, 0)
    }

    fn value_at_depth(value: &Value, depth: usize) -> Self {
        if depth >= SHAPE.depth {
            return Self::Unknown;
        }
        match value {
            Value::Object(fields) => Self::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::value_at_depth(value, depth + 1)))
                    .collect(),
            ),
            Value::Array(items) => Self::Array {
                item: Box::new(items.first().map_or(Self::Unknown, |value| {
                    Self::value_at_depth(value, depth + 1)
                })),
                length: Some(items.len()),
            },
            value => Self::Scalar {
                kind: match value {
                    Value::Null => "null",
                    Value::Bool(_) => "boolean",
                    Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
                    Value::Number(_) => "number",
                    _ => "string",
                },
                sample: Some(sample_text(value)),
            },
        }
    }

    pub fn field(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, shape)| shape),
            _ => None,
        }
    }

    pub fn item(&self) -> Option<&Self> {
        match self {
            Self::Array { item, .. } => Some(item),
            _ => None,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::Unknown => "unknown".into(),
            Self::Object(_) => "object".into(),
            Self::Array {
                length: Some(length),
                ..
            } => format!("array[{length}]"),
            Self::Array { .. } => "array".into(),
            Self::Scalar { kind, sample } => match sample {
                Some(sample) => format!("{kind}  {sample}"),
                None => (*kind).into(),
            },
        }
    }

    pub fn continues(&self) -> bool {
        matches!(self, Self::Object(_) | Self::Array { .. })
    }
}

fn sample_text(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => sample_string(value),
        _ => String::new(),
    }
}

fn sample_string(value: &str) -> String {
    let mut sample = String::new();
    let mut truncated = !push_sample(&mut sample, "\"");
    if !truncated {
        for ch in value.chars() {
            let escaped = match ch {
                '"' => "\\\"".into(),
                '\\' => "\\\\".into(),
                '\u{08}' => "\\b".into(),
                '\u{0c}' => "\\f".into(),
                '\n' => "\\n".into(),
                '\r' => "\\r".into(),
                '\t' => "\\t".into(),
                ch if ch.is_control() => format!("\\u{:04x}", ch as u32),
                ch => ch.to_string(),
            };
            if !push_sample(&mut sample, &escaped) {
                truncated = true;
                break;
            }
        }
    }
    if !truncated && !push_sample(&mut sample, "\"") {
        truncated = true;
    }
    if truncated {
        sample.push('…');
    }
    sample
}

fn push_sample(sample: &mut String, text: &str) -> bool {
    for ch in text.chars() {
        if sample.chars().count() >= SHAPE.sample_chars {
            return false;
        }
        sample.push(ch);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The completion shape and the real `--json` envelope are two
    /// descriptions of one thing. Tie them together here: adding a field to
    /// the envelope without adding it to the shape makes `--json --jq` offer
    /// candidates that the response does not have.
    #[test]
    fn the_completion_envelope_names_the_same_fields_as_the_real_one() {
        use crate::domain::functions::api_request::response_envelope;
        use crate::domain::types::http::HttpResponse;

        let real = response_envelope(&HttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        });
        let real_fields: Vec<&str> = real
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();

        let shape = JsonShape::response_envelope(JsonShape::Object(Vec::new()));
        let JsonShape::Object(fields) = &shape else {
            panic!("the envelope shape is an object: {shape:?}");
        };
        let shape_fields: Vec<&str> = fields.iter().map(|(name, _)| name.as_str()).collect();

        assert_eq!(
            shape_fields, real_fields,
            "`JsonShape::response_envelope` and `api_request::response_envelope` \
             must name the same fields, in the same order"
        );
    }
    use crate::domain::functions::{jq_completion, openapi::normalize_spec};
    use serde_json::json;

    fn schema(value: Value) -> Schema {
        normalize_spec(&json!({
            "openapi": "3.0.3", "info": {"title": "shape", "version": "1"},
            "paths": {"/": {"get": {"responses": {"200": {
                "description": "ok", "content": {"application/json": {"schema": value}}
            }}}}}
        }))
        .unwrap()
        .operations
        .remove(0)
        .response
        .unwrap()
        .schema
    }

    #[test]
    fn json_shape_producers_complete_the_same_object_and_array_paths() {
        let actual = JsonShape::from_value(&json!({
            "content": [{"id": 1, "name": "cat", "owner": {"active": true}}], "page": 1
        }));
        let declared = JsonShape::from_schema(&schema(json!({"properties": {
            "content": {"type": "array", "items": {"properties": {
                "id": {"type": "integer"}, "name": {"type": "string"},
                "owner": {"properties": {"active": {"type": "boolean"}}}
            }}}, "page": {"type": "integer"}
        }})));
        for path in [
            ".",
            ".content",
            ".content[].",
            ".content[0].na",
            ".content[].owner.",
            ".content[] | select(.na",
        ] {
            let candidates = |shape: &JsonShape| {
                jq_completion::complete(shape, path, path.len())
                    .candidates
                    .into_iter()
                    .map(|c| (c.value, c.continues))
                    .collect::<Vec<_>>()
            };
            assert_eq!(candidates(&declared), candidates(&actual), "{path}");
            assert!(!candidates(&declared).is_empty(), "{path}");
        }
        assert_eq!(declared.field("content").unwrap().description(), "array");
        assert_eq!(declared.field("page").unwrap().description(), "integer");
    }

    #[test]
    fn json_shape_schema_uses_structure_without_inventing_example_keys_or_samples() {
        for value in [
            json!({}),
            json!({"type": "object", "additionalProperties": {"type": "string"}}),
            json!({"default": {"invented": 1}, "example": {"alsoInvented": 2}}),
        ] {
            assert_eq!(JsonShape::from_schema(&schema(value)), JsonShape::Unknown);
        }
        let declared = JsonShape::from_schema(&schema(json!({
            "type": "string", "enum": ["first"], "default": "second", "example": "third"
        })));
        assert_eq!(
            declared,
            JsonShape::Scalar {
                kind: "string",
                sample: None
            }
        );
        let declared = JsonShape::from_schema(&schema(json!({"oneOf": [
            {"properties": {"first": {"type": "string"}}},
            {"properties": {"later": {"type": "boolean"}}}
        ]})));
        assert!(declared.field("first").is_some());
        assert!(declared.field("later").is_none());
    }

    #[test]
    fn json_shape_distinguishes_integers_and_bounds_scalar_samples() {
        assert_eq!(JsonShape::from_value(&json!(7)).description(), "integer  7");
        assert_eq!(
            JsonShape::from_value(&json!(1.5)).description(),
            "number  1.5"
        );
        let long = "x".repeat(100);
        let description = JsonShape::from_value(&json!(long)).description();
        assert_eq!(
            description.chars().count(),
            48 + "string  ".chars().count() + 1
        );
        assert!(description.ends_with('…'));
    }
}
