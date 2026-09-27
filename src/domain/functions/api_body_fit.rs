//! `kurama api --shape` / `--sample N`: a response body cut down to fit an
//! agent's context, applied to what is printed only.
//!
//! `--shape` replaces every value with its type: `"string"`, `"number"`,
//! `"boolean"` or `"null"`; an object keeps its keys; an array becomes
//! `{"array": <count>, "of": <element shape>}` (no `of` when empty), where
//! the elements' shapes are merged -- objects key by key, arrays into one
//! whose count is the total over the elements -- and shapes that still
//! differ are listed as a JSON array of alternatives.
//! `--sample N` keeps the first N elements of every array, at any depth, and
//! ends an array it cut with `{"…": {"omitted": M}}`. A body that is not JSON
//! is a string, as in the `--json` envelope: its shape is `"string"`, and a
//! sample leaves it alone.

use serde_json::{Map, Value, json};

use super::api_request::body_as_json;

/// Which cut `--shape` / `--sample` asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFit {
    Shape,
    Sample(u32),
}

/// The body as printed under `fit`; the request and the bytes `--output`
/// would write never pass through here.
pub fn fit_body(body: &[u8], fit: BodyFit) -> Vec<u8> {
    match (fit, body_as_json(body)) {
        (BodyFit::Shape, Some(value)) => shape_of(&value).to_json().to_string().into_bytes(),
        (BodyFit::Shape, None) => Shape::Scalar("string").to_json().to_string().into_bytes(),
        (BodyFit::Sample(keep), Some(value)) => {
            sample(&value, keep as usize).to_string().into_bytes()
        }
        (BodyFit::Sample(_), None) => body.to_vec(),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Scalar(&'static str),
    Object(Vec<(String, Shape)>),
    Array {
        count: usize,
        of: Option<Box<Shape>>,
    },
    OneOf(Vec<Shape>),
}

fn shape_of(value: &Value) -> Shape {
    match value {
        Value::Null => Shape::Scalar("null"),
        Value::Bool(_) => Shape::Scalar("boolean"),
        Value::Number(_) => Shape::Scalar("number"),
        Value::String(_) => Shape::Scalar("string"),
        Value::Object(fields) => Shape::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), shape_of(value)))
                .collect(),
        ),
        Value::Array(items) => Shape::Array {
            count: items.len(),
            of: items
                .iter()
                .map(shape_of)
                .reduce(Shape::merge)
                .map(Box::new),
        },
    }
}

impl Shape {
    /// One shape for two elements of an array: objects key by key, arrays
    /// into one array of both counts, the same scalar once, and anything
    /// else side by side.
    fn merge(self, other: Shape) -> Shape {
        match (self, other) {
            (Shape::OneOf(mut shapes), other) => {
                let alternatives = match other {
                    Shape::OneOf(others) => others,
                    other => vec![other],
                };
                for alternative in alternatives {
                    match shapes
                        .iter()
                        .position(|shape| shape.same_kind(&alternative))
                    {
                        Some(index) => {
                            let shape = shapes.remove(index);
                            shapes.insert(index, shape.merge(alternative));
                        }
                        None => shapes.push(alternative),
                    }
                }
                Shape::OneOf(shapes)
            }
            (Shape::Object(mut fields), Shape::Object(others)) => {
                for (key, shape) in others {
                    match fields.iter().position(|(name, _)| *name == key) {
                        Some(index) => {
                            let (name, known) = fields.remove(index);
                            fields.insert(index, (name, known.merge(shape)));
                        }
                        None => fields.push((key, shape)),
                    }
                }
                Shape::Object(fields)
            }
            (
                Shape::Array { count, of },
                Shape::Array {
                    count: more,
                    of: other,
                },
            ) => Shape::Array {
                count: count + more,
                of: match (of, other) {
                    (Some(of), Some(other)) => Some(Box::new(of.merge(*other))),
                    (of, other) => of.or(other),
                },
            },
            (Shape::Scalar(kind), Shape::Scalar(other)) if kind == other => Shape::Scalar(kind),
            (shape, other) => Shape::OneOf(vec![shape]).merge(other),
        }
    }

    fn same_kind(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Scalar(kind), Shape::Scalar(other)) => kind == other,
            (Shape::Object(_), Shape::Object(_)) | (Shape::Array { .. }, Shape::Array { .. }) => {
                true
            }
            _ => false,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Shape::Scalar(kind) => Value::String((*kind).into()),
            Shape::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, shape)| (key.clone(), shape.to_json()))
                    .collect(),
            ),
            Shape::Array { count, of: None } => json!({ "array": count }),
            Shape::Array {
                count,
                of: Some(of),
            } => json!({ "array": count, "of": of.to_json() }),
            Shape::OneOf(shapes) => Value::Array(shapes.iter().map(Shape::to_json).collect()),
        }
    }
}

fn sample(value: &Value, keep: usize) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), sample(value, keep)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => {
            let mut kept: Vec<Value> = items
                .iter()
                .take(keep)
                .map(|item| sample(item, keep))
                .collect();
            if items.len() > keep {
                kept.push(json!({ "…": { "omitted": items.len() - keep } }));
            }
            Value::Array(kept)
        }
        scalar => scalar.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaped(body: &str) -> Value {
        serde_json::from_slice(&fit_body(body.as_bytes(), BodyFit::Shape)).unwrap()
    }

    fn sampled(body: &str, keep: u32) -> Value {
        serde_json::from_slice(&fit_body(body.as_bytes(), BodyFit::Sample(keep))).unwrap()
    }

    #[test]
    fn a_shape_names_each_type_and_no_value() {
        let body = r#"{"login":"octocat","id":42,"admin":false,"bio":null,
            "repos":[{"name":"kurama","stars":10,"topics":["rust","cli"]},
                     {"name":"other","stars":3,"topics":[],"archived":true}],
            "matrix":[[1,2],[3]],"mixed":[1,"a",null,{"k":1}],"empty":[]}"#;
        let shape = shaped(body);
        assert_eq!(
            shape,
            json!({
                "login": "string",
                "id": "number",
                "admin": "boolean",
                "bio": "null",
                "repos": {"array": 2, "of": {
                    "name": "string",
                    "stars": "number",
                    "topics": {"array": 2, "of": "string"},
                    "archived": "boolean",
                }},
                "matrix": {"array": 2, "of": {"array": 3, "of": "number"}},
                "mixed": {"array": 4, "of": ["number", "string", "null", {"k": "number"}]},
                "empty": {"array": 0},
            })
        );
        let text = shape.to_string();
        for value in ["octocat", "42", "kurama", "rust", "cli", "other", "10"] {
            assert!(!text.contains(value), "{value} in {text}");
        }
    }

    #[test]
    fn a_shape_of_a_body_that_is_not_json_is_a_string() {
        assert_eq!(shaped("plain text"), json!("string"));
        assert_eq!(shaped("[]"), json!({"array": 0}));
        assert_eq!(shaped("7"), json!("number"));
    }

    #[test]
    fn a_sample_cuts_every_array_and_counts_what_it_cut() {
        let body = r#"{"items":[{"id":1,"tags":["a","b","c"]},{"id":2,"tags":["d"]},{"id":3,"tags":[]}],
            "total":3,"pair":[1,2]}"#;
        assert_eq!(
            sampled(body, 2),
            json!({
                "items": [
                    {"id": 1, "tags": ["a", "b", {"…": {"omitted": 1}}]},
                    {"id": 2, "tags": ["d"]},
                    {"…": {"omitted": 1}},
                ],
                "total": 3,
                "pair": [1, 2],
            })
        );
        assert_eq!(sampled("[[1,2],[3]]", 0), json!([{"…": {"omitted": 2}}]));
    }

    #[test]
    fn a_sample_keeps_a_body_that_is_not_json_and_the_key_order() {
        assert_eq!(fit_body(b"plain", BodyFit::Sample(1)), b"plain");
        assert_eq!(
            fit_body(br#"{"z":1,"a":[1,2]}"#, BodyFit::Sample(1)),
            r#"{"z":1,"a":[1,{"…":{"omitted":1}}]}"#.as_bytes()
        );
    }
}
