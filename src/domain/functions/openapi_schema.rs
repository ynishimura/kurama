//! The reduced `Schema` of a JSON Schema object inside an OpenAPI document:
//! local `$ref`s and their chains followed to a bounded depth, a cycle
//! ended in a bare object, `allOf` merged and the first `oneOf` / `anyOf`
//! alternative kept. Every reference that does not resolve leaves one
//! warning line, and every other reduction a `SchemaLimitation` on the
//! schema it reduced.

use serde_json::{Map, Value};

use super::openapi::{MAX_SCHEMA_DEPTH, sanitize_text, sanitize_value};
use crate::domain::types::api_spec::{Property, Schema, SchemaLimitation};

/// The document references resolve in, and the warnings reading it left.
pub(super) struct Document<'d> {
    value: &'d Value,
    pub(super) warnings: Vec<String>,
}

impl<'d> Document<'d> {
    pub(super) fn new(value: &'d Value) -> Self {
        Self {
            value,
            warnings: Vec::new(),
        }
    }

    pub(super) fn value(&self) -> &'d Value {
        self.value
    }

    pub(super) fn warn(&mut self, line: String) {
        let line = sanitize_text(&line);
        if !self.warnings.contains(&line) {
            self.warnings.push(line);
        }
    }

    /// The value a local `#/...` reference points at.
    fn lookup(&self, reference: &str) -> Option<&'d Value> {
        let pointer = reference.strip_prefix('#')?;
        self.value.pointer(pointer)
    }

    /// `value` with a `$ref` followed; `None` for a reference that does
    /// not resolve (a warning is recorded).
    pub(super) fn deref(&mut self, value: &'d Value) -> Option<&'d Value> {
        let mut current = value;
        for _ in 0..MAX_SCHEMA_DEPTH {
            let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
                return Some(current);
            };
            match self.lookup(reference) {
                Some(target) => current = target,
                None => {
                    self.warn(format!("$ref {reference} is not resolved"));
                    return None;
                }
            }
        }
        Some(current)
    }

    /// The reduced schema of a schema object at the top of an operation.
    pub(super) fn root_schema(&mut self, value: &'d Value) -> Schema {
        self.schema(value, 0, &mut Vec::new())
    }

    /// The reduced schema of `value`, `$ref`s followed; `stack` holds the
    /// references being followed, so a cycle ends in a bare object. A chain
    /// of references is followed in a loop and counts toward the depth
    /// bound on its own, so a long acyclic chain that adds no object level
    /// ends in a limitation instead of recursing without a bound.
    pub(super) fn schema(
        &mut self,
        value: &'d Value,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Schema {
        let outer = stack.len();
        let mut current = value;
        let schema = loop {
            let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
                break match current.as_object() {
                    Some(object) => self.schema_fields(object, depth, stack),
                    None => Schema::default(),
                };
            };
            if stack.iter().any(|seen| seen == reference) {
                break Schema {
                    type_name: "object".into(),
                    limitations: vec![SchemaLimitation::Cycle {
                        reference: sanitize_text(reference),
                    }],
                    ..Schema::default()
                };
            }
            if stack.len() - outer >= MAX_SCHEMA_DEPTH {
                break Schema {
                    limitations: vec![SchemaLimitation::DepthBound],
                    ..Schema::default()
                };
            }
            match self.lookup(reference) {
                Some(target) => {
                    stack.push(reference.to_string());
                    current = target;
                }
                None => {
                    self.warn(format!("$ref {reference} is not resolved"));
                    break Schema {
                        unresolved_ref: Some(sanitize_text(reference)),
                        ..Schema::default()
                    };
                }
            }
        };
        stack.truncate(outer);
        schema
    }

    pub(super) fn schema_fields(
        &mut self,
        object: &'d Map<String, Value>,
        depth: usize,
        stack: &mut Vec<String>,
    ) -> Schema {
        let mut schema = Schema::default();
        // Past the bound nothing nested is read, a combinator's parts
        // included: each part may be a `$ref` that adds no object level.
        let bounded = depth >= MAX_SCHEMA_DEPTH;
        if bounded {
            if ["items", "properties", "allOf", "oneOf", "anyOf"]
                .iter()
                .any(|key| object.contains_key(*key))
            {
                schema.limitations.push(SchemaLimitation::DepthBound);
            }
        } else if let Some(parts) = object.get("allOf").and_then(Value::as_array) {
            // `{"required": [...]}` alone tightens the other parts, in any
            // order: the names are applied once every part is merged.
            let mut names = Vec::new();
            for part in parts {
                let merged = self.schema(part, depth + 1, stack);
                merge(&mut schema, merged);
                if let Some(part) = part.as_object() {
                    names.extend(required_names(part));
                }
            }
            mark_required(&mut schema, &names);
        } else if let Some((keyword, options)) =
            ["oneOf", "anyOf"].into_iter().find_map(|keyword| {
                object
                    .get(keyword)
                    .and_then(Value::as_array)
                    .filter(|options| !options.is_empty())
                    .map(|options| (keyword, options))
            })
        {
            schema = self.schema(&options[0], depth + 1, stack);
            if options.len() > 1 {
                schema
                    .limitations
                    .push(SchemaLimitation::FirstAlternativeOnly {
                        keyword,
                        alternatives: options.len(),
                    });
            }
        }
        if let Some((type_name, mut others)) = type_name(object.get("type")) {
            schema.type_name = type_name;
            // 3.0 says with `nullable` what 3.1 says with a `null` entry.
            if object.get("nullable") == Some(&Value::Bool(true))
                && !others.iter().any(|t| t == "null")
            {
                others.push("null".into());
            }
            if !others.is_empty() {
                schema
                    .limitations
                    .push(SchemaLimitation::OtherTypes { types: others });
            }
        }
        if let Some(values) = object.get("enum").and_then(Value::as_array) {
            schema.enum_values = values.iter().map(sanitize_value).collect();
        }
        if let Some(default) = object.get("default") {
            schema.default = Some(sanitize_value(default));
        }
        if let Some(example) = object.get("example").or_else(|| {
            object
                .get("examples")
                .and_then(Value::as_array)
                .and_then(|examples| examples.first())
        }) {
            schema.example = Some(sanitize_value(example));
        }
        if bounded {
            return schema;
        }
        if let Some(items) = object.get("items") {
            schema.items = Some(Box::new(self.schema(items, depth + 1, stack)));
            if schema.type_name.is_empty() {
                schema.type_name = "array".into();
            }
        }
        let required = required_names(object);
        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                let name = sanitize_text(name);
                let property = Property {
                    name: name.clone(),
                    required: required.contains(&name),
                    schema: self.schema(property, depth + 1, stack),
                };
                match schema.properties.iter().position(|p| p.name == name) {
                    Some(index) => schema.properties[index] = property,
                    None => schema.properties.push(property),
                }
            }
            if schema.type_name.is_empty() {
                schema.type_name = "object".into();
            }
        }
        mark_required(&mut schema, &required);
        schema
    }
}

/// The `required` list of a schema object.
fn required_names(object: &Map<String, Value>) -> Vec<String> {
    object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_text)
                .collect()
        })
        .unwrap_or_default()
}

/// Mark the properties `names` as required (a `required` list may come
/// from an `allOf` part that declares no property of its own).
fn mark_required(schema: &mut Schema, names: &[String]) {
    for property in &mut schema.properties {
        if names.contains(&property.name) {
            property.required = true;
        }
    }
}

/// `type` as a string, or the first non-null entry of a 3.1 type array
/// with the entries it leaves out.
fn type_name(value: Option<&Value>) -> Option<(String, Vec<String>)> {
    match value? {
        Value::String(name) => Some((sanitize_text(name), Vec::new())),
        Value::Array(names) => {
            let names: Vec<String> = names
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_text)
                .collect();
            let kept = names.iter().position(|name| name != "null")?;
            let mut others = names;
            let kept = others.remove(kept);
            Some((kept, others))
        }
        _ => None,
    }
}

/// `allOf`: properties, required flags and scalar facets of `part` added
/// to `schema`.
fn merge(schema: &mut Schema, part: Schema) {
    if schema.type_name.is_empty() {
        schema.type_name = part.type_name;
    }
    if schema.enum_values.is_empty() {
        schema.enum_values = part.enum_values;
    }
    if schema.default.is_none() {
        schema.default = part.default;
    }
    if schema.example.is_none() {
        schema.example = part.example;
    }
    if schema.items.is_none() {
        schema.items = part.items;
    }
    schema.limitations.extend(part.limitations);
    for mut property in part.properties {
        match schema
            .properties
            .iter()
            .position(|existing| existing.name == property.name)
        {
            // A part that declares a property again keeps it required.
            Some(index) => {
                property.required |= schema.properties[index].required;
                schema.properties[index] = property;
            }
            None => schema.properties.push(property),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `wrap` applied `levels` times around an object with one property.
    fn nested(levels: usize, wrap: impl Fn(Value) -> Value) -> Value {
        (0..levels).fold(
            json!({"type": "object", "properties": {"deep": {"type": "string"}}}),
            |inner, _| wrap(inner),
        )
    }

    fn reduced(value: &Value) -> Schema {
        let document = json!({});
        Document::new(&document).root_schema(value)
    }

    fn innermost(mut schema: &Schema) -> &Schema {
        while let Some(items) = &schema.items {
            schema = items;
        }
        schema
    }

    /// A document whose `components/schemas/s0` is a chain of `hops`
    /// references ending in an object with one property.
    fn ref_chain(hops: usize) -> Value {
        let mut schemas = Map::new();
        for hop in 0..hops {
            schemas.insert(
                format!("s{hop}"),
                json!({"$ref": format!("#/components/schemas/s{}", hop + 1)}),
            );
        }
        schemas.insert(
            format!("s{hops}"),
            json!({"type": "object", "properties": {"deep": {"type": "string"}}}),
        );
        json!({"components": {"schemas": schemas}})
    }

    fn reduced_chain(hops: usize) -> Schema {
        let document = ref_chain(hops);
        let start = json!({"$ref": "#/components/schemas/s0"});
        Document::new(&document).root_schema(&start)
    }

    #[test]
    fn a_reference_chain_up_to_the_depth_bound_is_followed_to_its_end() {
        // `start` itself is the first hop.
        let schema = reduced_chain(MAX_SCHEMA_DEPTH - 1);
        assert_eq!(schema.type_name, "object");
        assert_eq!(schema.properties[0].name, "deep");
        assert!(schema.limitations.is_empty(), "{schema:?}");
    }

    #[test]
    fn a_reference_chain_past_the_depth_bound_ends_as_a_limitation() {
        let schema = reduced_chain(MAX_SCHEMA_DEPTH);
        assert!(schema.properties.is_empty(), "{schema:?}");
        assert_eq!(schema.limitations, [SchemaLimitation::DepthBound]);
    }

    #[test]
    fn a_very_long_reference_chain_does_not_exhaust_the_stack() {
        let schema = reduced_chain(200_000);
        assert_eq!(schema.limitations, [SchemaLimitation::DepthBound]);
    }

    #[test]
    fn a_very_long_chain_through_combinators_does_not_exhaust_the_stack() {
        for keyword in ["allOf", "oneOf", "anyOf"] {
            let mut schemas = Map::new();
            for hop in 0..200_000 {
                schemas.insert(
                    format!("s{hop}"),
                    json!({keyword: [{"$ref": format!("#/components/schemas/s{}", hop + 1)}]}),
                );
            }
            let document = json!({"components": {"schemas": schemas}});
            let start = json!({"$ref": "#/components/schemas/s0"});
            let schema = Document::new(&document).root_schema(&start);
            assert_eq!(
                schema.limitations,
                [SchemaLimitation::DepthBound],
                "{keyword}"
            );
        }
    }

    #[test]
    fn a_cycle_inside_a_reference_chain_is_still_a_cycle() {
        let document = json!({"components": {"schemas": {
            "a": {"$ref": "#/components/schemas/b"},
            "b": {"$ref": "#/components/schemas/a"},
        }}});
        let start = json!({"$ref": "#/components/schemas/a"});
        let schema = Document::new(&document).root_schema(&start);
        assert_eq!(
            schema.limitations,
            [SchemaLimitation::Cycle {
                reference: "#/components/schemas/a".into()
            }]
        );
    }

    #[test]
    fn schema_reading_stops_at_the_depth_bound_through_every_kind_of_nesting() {
        let past_the_bound = MAX_SCHEMA_DEPTH + 2;
        for wrap in [
            |inner| json!({"allOf": [inner]}),
            |inner| json!({"oneOf": [inner]}),
            |inner| json!({"anyOf": [inner]}),
        ] {
            let schema = reduced(&nested(past_the_bound, wrap));
            assert!(schema.properties.is_empty(), "{schema:?}");
            assert!(!reduced(&nested(1, wrap)).properties.is_empty());
        }
        let schema = reduced(&nested(
            past_the_bound,
            |inner| json!({"type": "array", "items": inner}),
        ));
        let deepest = innermost(&schema);
        assert!(deepest.properties.is_empty(), "{schema:?}");
        assert_eq!(deepest.type_name, "array");
    }
}
