//! Normalizer regression tests for supported API descriptions and terminal-safe output.

use super::*;
use serde_json::json;

/// Keys whose value the document uses to mean something rather than to say
/// something: tainting them would change what the normalizer builds instead
/// of testing what it copies.
const STRUCTURAL_KEYS: [&str; 8] = [
    "type", "$ref", "format", "in", "openapi", "swagger", "required", "style",
];

/// The same marker in every string the document says rather than means.
fn taint(value: &Value, key: Option<&str>) -> Value {
    match value {
        Value::String(text) if !key.is_some_and(|key| STRUCTURAL_KEYS.contains(&key)) => {
            Value::String(format!("{text}\u{1b}[31m\u{202e}\u{feff}"))
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), taint(value, Some(name))))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|item| taint(item, key)).collect()),
        other => other.clone(),
    }
}

/// Walking the document beats a hand-written fixture: a string the
/// normalizer starts copying -- a field added to `Operation`, a new part of
/// a `Response` -- is covered here without anyone remembering to taint it.
#[test]
fn no_string_a_document_says_reaches_the_spec_unsanitized() {
    let document: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/openapi/petstore.json"
    ))
    .unwrap();
    let clean = normalize_spec(&document).unwrap();
    let spec = normalize_spec(&taint(&document, None)).unwrap();
    assert_eq!(
        spec.operations.len(),
        clean.operations.len(),
        "the tainted document must still normalize, or this test proves nothing"
    );
    assert!(!spec.operations.is_empty());
    let debug = format!("{spec:?}");
    for escaped in ["\\u{1b}", "\\u{202e}", "\\u{feff}"] {
        assert!(
            !debug.contains(escaped),
            "{escaped} survived normalization: {debug}"
        );
    }
}

#[test]
fn all_document_strings_are_terminal_safe_and_descriptions_keep_paragraphs() {
    let document: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/openapi/control-characters.json"
    ))
    .unwrap();
    let spec = normalize_spec(&document).unwrap();
    let debug = format!("{spec:?}");
    for escaped in [
        "\\u{1b}",
        "\\u{7}",
        "\\u{85}",
        "\\t",
        "\\u{200b}",
        "\\u{202e}",
        "\\u{2066}",
        "\\u{feff}",
    ] {
        assert!(!debug.contains(escaped), "control {escaped} in {debug}");
    }
    assert_eq!(spec.description.as_deref(), Some("About\n\nPets"));
    assert_eq!(
        spec.operations[0].description.as_deref(),
        Some("First paragraph\n\nSecond paragraph")
    );
    assert_eq!(spec.operations[0].id, "pets/list");
    assert_eq!(spec.operations[1].id, "POST /pets");
    assert_eq!(spec.operations[1].summary.as_deref(), Some("Create pet"));
    let body = spec.operations[0].request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/json");
    assert_eq!(body.schema.skeleton(), json!({"name": "cat"}));
    assert_eq!(
        body.schema.properties[1]
            .schema
            .items
            .as_ref()
            .unwrap()
            .default,
        Some(json!({"key": ["value"]}))
    );
    assert_eq!(spec.warnings.len(), 2);
}

#[test]
fn swagger_strings_and_nested_examples_are_terminal_safe() {
    let document = json!({
        "swagger": "2.0", "info": {"title": "Swagger\u{1b}", "version": "1\u{7}"},
        "schemes": ["https\u{7}"], "host": "pets.example\u{1b}", "basePath": "/v1\u{85}",
        "consumes": ["application/json\u{1b}"],
        "paths": {"/pets": {"post": {"parameters": [
            {"name": "body", "in": "body", "description": "body\u{7}\nmore", "schema": {"type": ["null", "string\u{1b}"]}},
            {"name": "q\u{1b}", "in": "query", "type": "string", "x-example": {"a\u{7}": ["b\u{1b}"]}},
            {"name": "file\u{7}", "in": "formData"}
        ]}}}
    });
    let spec = normalize_spec(&document).unwrap();
    let debug = format!("{spec:?}");
    for escaped in ["\\u{1b}", "\\u{7}", "\\u{85}"] {
        assert!(!debug.contains(escaped), "{debug}");
    }
    assert_eq!(spec.server.as_deref(), Some("https://pets.example/v1"));
    assert_eq!(
        spec.operations[0].parameters[0].schema.example,
        Some(json!({"a": ["b"]}))
    );
}

#[test]
fn openapi_response_selection_and_shape_preserve_optional_fields() {
    use crate::domain::functions::spec_output::operation_json;
    let document = json!({
        "openapi": "3.0.3", "info": {"title": "responses", "version": "1"},
        "paths": {"/pets": {"get": {"responses": {
            "201": {"description": "created", "content": {"application/json": {"schema": {"type": "boolean"}}}},
            "200": {"$ref": "#/components/responses/Page"},
            "default": {"description": "fallback", "content": {"application/json": {"schema": {"type": "string"}}}}
        }}}},
        "components": {"responses": {"Page": {"description": "A page\u{1b}\n\nDetails", "content": {
            "text/plain": {"schema": {"type": "string"}},
            "application/json": {"schema": {"type": "object", "properties": {
                "content": {"type": "array", "items": {"$ref": "#/components/schemas/Pet"}},
                "page": {"type": "integer"}
            }}}
        }}}, "schemas": {"Pet": {"type": "object", "required": ["id"], "properties": {
            "id": {"type": "integer"}, "name": {"type": "string", "example": "not a type"},
            "parent": {"$ref": "#/components/schemas/Pet"}
        }}}}
    });
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(
        operation_json("pets", &spec.operations[0])["response"],
        json!({
            "status": "200", "content_type": "application/json", "description": "A page\n\nDetails",
            "shape": {"content": [{"id": "integer", "name": "string", "parent": {}}], "page": "integer"}
        })
    );
}

#[test]
fn openapi_response_selection_prefers_json_suffix_media_types() {
    let document = json!({
        "openapi": "3.0.3", "paths": {"/pets": {"get": {"responses": {
            "200": {"description": "ok", "content": {
                "text/csv": {"schema": {"type": "string"}},
                "application/vnd.api+json": {"schema": {"type": "object", "properties": {
                    "id": {"type": "integer"}
                }}}
            }}
        }}}}
    });
    let spec = normalize_spec(&document).unwrap();
    let response = spec.operations[0].response.as_ref().unwrap();
    assert_eq!(response.content_type, "application/vnd.api+json");
    assert_eq!(response.schema.shape(), json!({"id": "integer"}));
}

#[test]
fn openapi_response_selection_follows_success_priority_and_default() {
    use crate::domain::functions::spec_output::operation_json;
    for (statuses, expected) in [
        (vec!["default", "202", "201"], Some("201")),
        (vec!["default", "206", "202"], Some("206")),
        (vec!["400", "default"], Some("default")),
        (vec!["400", "500"], None),
    ] {
        let responses: Map<String, Value> = statuses.into_iter().map(|status| (status.into(), json!({
            "description": "ok", "content": {"text/plain": {"schema": {"type": "string"}}}
        }))).collect();
        let document = json!({"openapi":"3.0.3", "paths":{"/x":{"get":{"responses":responses}}}});
        let spec = normalize_spec(&document).unwrap();
        let response = operation_json("x", &spec.operations[0])["response"].clone();
        assert_eq!(response["status"].as_str(), expected, "{responses:?}");
        if expected.is_some() {
            assert_eq!(response["content_type"], "text/plain");
        }
    }
}

#[test]
fn openapi_response_schema_supports_swagger_produces_and_missing_body() {
    use crate::domain::functions::spec_output::operation_json;
    let document = json!({"swagger":"2.0", "produces":["text/plain", "application/json"],
        "paths":{"/x":{"get":{"responses":{"200":{"description":"ok", "schema":{"type":"array", "items":{"$ref":"#/definitions/Pet"}}}}},
        "post":{"produces":["application/xml"], "responses":{"201":{"schema":{"type":"string"}}}},
        "delete":{"responses":{"204":{"description":"no body"}}}}},
        "definitions":{"Pet":{"type":"object", "properties":{"id":{"type":"integer"}}}}});
    let spec = normalize_spec(&document).unwrap();
    let response = operation_json("x", &spec.operations[0])["response"].clone();
    assert_eq!(response["content_type"], "application/json");
    assert_eq!(response["shape"], json!([{"id":"integer"}]));
    assert_eq!(
        operation_json("x", &spec.operations[1])["response"]["content_type"],
        "application/xml"
    );
    assert!(operation_json("x", &spec.operations[2])["response"].is_null());
}

fn petstore_v3() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {"title": "Petstore", "version": "1.2.0", "description": "Pets"},
        "servers": [{"url": "https://petstore.example.com/v1"}],
        "security": [{"oauth2": ["read:pets"]}],
        "paths": {
            "/pets": {
                "get": {
                    "operationId": "pets/list",
                    "summary": "List pets",
                    "tags": ["pets"],
                    "parameters": [
                        {"name": "limit", "in": "query", "schema": {"type": "integer", "default": 20}},
                        {"$ref": "#/components/parameters/state"}
                    ]
                },
                "post": {
                    "operationId": "pets/create",
                    "summary": "Create a pet",
                    "tags": ["pets"],
                    "security": [{"oauth2": ["write:pets", "read:pets"]}, {"api_key": []}],
                    "requestBody": {"$ref": "#/components/requestBodies/NewPet"},
                    "externalDocs": {"url": "https://docs.example.com/pets#create"}
                }
            },
            "/pets/{petId}": {
                "parameters": [
                    {"name": "petId", "in": "path", "required": true, "schema": {"type": "string"}, "description": "The pet"}
                ],
                "get": {
                    "operationId": "pets/get",
                    "parameters": [
                        {"name": "X-Trace", "in": "header", "schema": {"type": "string"}},
                        {"name": "petId", "in": "path", "required": true, "schema": {"type": "string", "example": "p-1"}}
                    ]
                },
                "delete": {
                    "security": [],
                    "deprecated": true
                }
            }
        },
        "components": {
            "parameters": {
                "state": {"name": "state", "in": "query", "required": false, "schema": {"type": "string", "enum": ["available", "sold"]}}
            },
            "requestBodies": {
                "NewPet": {
                    "required": true,
                    "description": "The pet to add",
                    "content": {
                        "application/xml": {"schema": {"type": "string"}},
                        "application/json": {"schema": {"$ref": "#/components/schemas/NewPet"}}
                    }
                }
            },
            "schemas": {
                "Named": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}},
                "NewPet": {
                    "allOf": [
                        {"$ref": "#/components/schemas/Named"},
                        {"type": "object", "properties": {
                            "tags": {"type": "array", "items": {"type": "string"}},
                            "owner": {"$ref": "#/components/schemas/Owner"},
                            "external": {"$ref": "https://schemas.example.com/x.json#/X"},
                            "self": {"$ref": "#/components/schemas/NewPet"}
                        }}
                    ]
                },
                "Owner": {"type": "object", "properties": {"login": {"type": "string", "example": "octocat"}}}
            }
        }
    })
}

#[test]
fn v3_operations_parameters_bodies_and_scopes_are_normalized() {
    let spec = normalize_spec(&petstore_v3()).unwrap();
    assert_eq!(spec.title, "Petstore");
    assert_eq!(spec.version, "1.2.0");
    assert_eq!(spec.description.as_deref(), Some("Pets"));
    assert_eq!(
        spec.server.as_deref(),
        Some("https://petstore.example.com/v1")
    );
    let ids: Vec<&str> = spec.operations.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "pets/list",
            "pets/create",
            "pets/get",
            "DELETE /pets/{petId}"
        ]
    );

    let list = &spec.operations[0];
    assert_eq!(list.method, "GET");
    assert_eq!(list.summary.as_deref(), Some("List pets"));
    assert_eq!(list.tags, ["pets"]);
    assert_eq!(
        list.scopes,
        ["read:pets"],
        "inherits the document's security"
    );
    let names: Vec<(&str, ParameterLocation, bool)> = list
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location, p.required))
        .collect();
    assert_eq!(
        names,
        [
            ("limit", ParameterLocation::Query, false),
            ("state", ParameterLocation::Query, false)
        ]
    );
    assert_eq!(list.parameters[0].schema.default, Some(json!(20)));
    assert_eq!(list.parameters[1].schema.display_type(), "available|sold");

    let create = &spec.operations[1];
    assert_eq!(create.scopes, ["read:pets", "write:pets"]);
    assert_eq!(
        create.external_docs.as_deref(),
        Some("https://docs.example.com/pets#create")
    );
    let body = create.request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/json");
    assert!(body.required);
    assert_eq!(body.description.as_deref(), Some("The pet to add"));
    assert_eq!(body.schema.type_name, "object");
    let properties: Vec<(&str, bool)> = body
        .schema
        .properties
        .iter()
        .map(|p| (p.name.as_str(), p.required))
        .collect();
    assert_eq!(
        properties,
        [
            ("name", true),
            ("tags", false),
            ("owner", false),
            ("external", false),
            ("self", false)
        ],
        "allOf parts in order, properties in document order"
    );
    assert_eq!(
        body.schema.skeleton(),
        json!({"name": ""}),
        "only the required property; optional ones are named separately"
    );
    assert_eq!(
        body.schema.optional_properties(),
        ["tags", "owner", "external", "self"]
    );
    let external = &body.schema.properties[3];
    assert_eq!(
        external.schema.unresolved_ref.as_deref(),
        Some("https://schemas.example.com/x.json#/X")
    );

    let get = &spec.operations[2];
    assert_eq!(get.path, "/pets/{petId}");
    let names: Vec<(&str, ParameterLocation, bool)> = get
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location, p.required))
        .collect();
    assert_eq!(
        names,
        [
            ("petId", ParameterLocation::Path, true),
            ("X-Trace", ParameterLocation::Header, false)
        ],
        "the operation's petId replaces the path-level one, in place"
    );
    assert_eq!(get.parameters[0].schema.example, Some(json!("p-1")));

    let delete = &spec.operations[3];
    assert!(!delete.has_operation_id);
    assert!(delete.deprecated);
    assert!(delete.scopes.is_empty(), "`security: []` means none");
    assert_eq!(delete.parameters[0].description.as_deref(), Some("The pet"));

    assert_eq!(
        spec.warnings,
        ["$ref https://schemas.example.com/x.json#/X is not resolved"]
    );
}

#[test]
fn all_of_parts_tighten_required_in_any_order_and_a_dangling_local_ref_is_reported() {
    let document = json!({
        "openapi": "3.0.0",
        "info": {"title": "T", "version": 1.0},
        "paths": {"/x": {"post": {
            "operationId": "x",
            "requestBody": {"content": {"application/json": {"schema": {
                "allOf": [
                    {"required": ["note"]},
                    {"$ref": "#/components/schemas/Base"},
                    {"$ref": "#/components/schemas/Memver"},
                    {"properties": {"id": {"type": "integer", "description": "the id"}}}
                ]
            }}}}
        }}},
        "components": {"schemas": {
            "Base": {
                "type": "object",
                "required": ["id"],
                "properties": {"id": {"type": "integer"}, "note": {"type": "string"}, "tag": {"type": "string"}}
            }
        }}
    });
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(spec.version, "1.0", "a YAML number is still a version");
    let body = spec.operations[0].request_body.as_ref().unwrap();
    assert_eq!(
        body.schema.skeleton(),
        json!({"id": 0, "note": ""}),
        "a required list before the properties, and a property declared again, keep the flag"
    );
    assert_eq!(body.schema.optional_properties(), ["tag"]);
    assert_eq!(
        spec.warnings,
        ["$ref #/components/schemas/Memver is not resolved"]
    );
}

#[test]
fn v2_host_base_path_body_parameters_and_form_data_are_handled() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "Legacy", "version": "1"},
        "host": "api.example.com",
        "basePath": "/v2",
        "schemes": ["https", "http"],
        "consumes": ["application/json"],
        "securityDefinitions": {"oauth": {"type": "oauth2", "scopes": {"repo": "x"}}},
        "paths": {
            "/items": {
                "post": {
                    "operationId": "createItem",
                    "consumes": ["application/vnd.item+json", "text/plain"],
                    "security": [{"oauth": ["repo"]}],
                    "parameters": [
                        {"name": "item", "in": "body", "required": true, "schema": {"$ref": "#/definitions/Item"}},
                        {"name": "X-Team", "in": "header", "type": "string", "x-example": "core"}
                    ]
                },
                "get": {
                    "parameters": [
                        {"$ref": "#/parameters/page"},
                        {"name": "kind", "in": "query", "type": "string", "enum": ["a", "b"], "required": true}
                    ]
                }
            },
            "/upload": {
                "post": {
                    "operationId": "upload",
                    "parameters": [
                        {"name": "file", "in": "formData", "type": "file", "required": true},
                        {"name": "session", "in": "cookie", "type": "string"}
                    ]
                }
            }
        },
        "parameters": {
            "page": {"name": "page", "in": "query", "type": "integer", "default": 1}
        },
        "definitions": {
            "Item": {"type": "object", "required": ["name"], "properties": {
                "name": {"type": "string"},
                "price": {"type": "number", "format": "float"},
                "parent": {"$ref": "#/definitions/Item"}
            }}
        }
    });
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(spec.server.as_deref(), Some("https://api.example.com/v2"));
    let ids: Vec<&str> = spec.operations.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, ["GET /items", "createItem", "upload"]);

    let list = &spec.operations[0];
    assert_eq!(list.parameters[0].name, "page");
    assert_eq!(list.parameters[0].schema.type_name, "integer");
    assert_eq!(list.parameters[0].schema.default, Some(json!(1)));
    assert!(list.parameters[1].required);
    assert_eq!(
        list.parameters[1].schema.enum_values,
        vec![json!("a"), json!("b")]
    );

    let create = &spec.operations[1];
    assert_eq!(create.scopes, ["repo"]);
    let body = create.request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/vnd.item+json");
    assert!(body.required);
    assert_eq!(body.schema.skeleton(), json!({"name": ""}));
    assert_eq!(body.schema.optional_properties(), ["price", "parent"]);
    assert_eq!(create.parameters[0].name, "X-Team");
    assert_eq!(create.parameters[0].schema.example, Some(json!("core")));

    let upload = &spec.operations[2];
    assert_eq!(
        upload.unsupported,
        ["formData parameter 'file'", "cookie parameter 'session'"]
    );
    assert!(upload.parameters.is_empty());
    assert_eq!(
        spec.warnings,
        [
            "upload: formData parameter 'file' is not supported",
            "upload: cookie parameter 'session' is not supported"
        ]
    );
}

#[test]
fn v31_type_arrays_examples_and_nested_depth_are_bounded() {
    let mut nested = json!({"type": "string"});
    for level in 0..10 {
        nested = json!({"type": "object", "properties": {format!("level{level}"): nested}});
    }
    let document = json!({
        "openapi": "3.1.0",
        "info": {"title": "T"},
        "paths": {
            "/x": {
                "get": {
                    "operationId": "x",
                    "parameters": [
                        {"name": "q", "in": "query", "schema": {"type": ["null", "string"], "examples": ["hello"]}}
                    ],
                    "requestBody": {"content": {"text/plain": {"schema": nested}}}
                }
            }
        }
    });
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(spec.version, "");
    let operation = &spec.operations[0];
    assert_eq!(operation.parameters[0].schema.type_name, "string");
    assert_eq!(operation.parameters[0].schema.example, Some(json!("hello")));
    let body = operation.request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "text/plain");
    let mut depth = 0;
    let mut schema = &body.schema;
    while let Some(property) = schema.properties.first() {
        depth += 1;
        schema = &property.schema;
    }
    assert_eq!(depth, MAX_SCHEMA_DEPTH);
    assert_eq!(schema.type_name, "object", "deeper levels keep their type");
}

#[test]
fn newer_openapi_three_minor_versions_are_accepted() {
    let document = json!({
        "openapi": "3.2.0",
        "info": {"title": "Future", "version": "1"},
        "servers": [{"url": "https://future.example.com"}],
        "paths": {"/health": {"get": {"operationId": "health"}}}
    });
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(spec.server.as_deref(), Some("https://future.example.com"));
    assert_eq!(spec.operations[0].id, "health");
}

#[test]
fn a_numeric_swagger_version_is_accepted() {
    let document = json!({"swagger": 2.0, "info": {"title": "L"}, "paths": {}});
    let spec = normalize_spec(&document).unwrap();
    assert!(spec.operations.is_empty());
    assert_eq!(spec.version, "");
}

#[test]
fn unsupported_documents_are_refused_with_the_reason() {
    let cases = [
        (json!({"info": {}}), "neither `openapi` nor `swagger`"),
        (
            json!({"openapi": "4.0.0"}),
            "unsupported OpenAPI version \"4.0.0\"; OpenAPI 3.x and Swagger 2.0 are supported",
        ),
        (
            json!({"swagger": "1.2"}),
            "unsupported Swagger version \"1.2\"; OpenAPI 3.x and Swagger 2.0 are supported",
        ),
        (json!({"openapi": "3.0.0"}), "no `paths` object"),
        (json!([1, 2]), "not a JSON object"),
    ];
    for (document, expected) in cases {
        let error = normalize_spec(&document).unwrap_err();
        assert!(error.contains(expected), "{document}: {error}");
    }
}

#[test]
fn scopes_and_media_types_are_picked_deterministically() {
    assert_eq!(
        scopes_of(&json!([{"a": ["z", "b"]}, {"c": ["b"]}, {"d": []}])),
        ["b", "z"]
    );
    assert_eq!(scopes_of(&json!("nonsense")), Vec::<String>::new());
    assert_eq!(
        pick_media_type(["text/plain", "application/vnd.github+json"].into_iter()),
        Some("application/vnd.github+json".into())
    );
    assert_eq!(
        pick_media_type(["text/plain", "application/json"].into_iter()),
        Some("application/json".into())
    );
    assert_eq!(
        pick_media_type(["text/plain"].into_iter()),
        Some("text/plain".into())
    );
    assert_eq!(pick_media_type(std::iter::empty()), None);
}

#[test]
fn openapi_response_shape_names_scalar_types_even_with_enums() {
    let document = json!({"openapi":"3.0.3","paths":{"/x":{"get":{"responses":{"200":{
        "content":{"application/json":{"schema":{"type":"object","properties":{
            "state":{"type":"integer","enum":[0,1]},
            "label":{"type":"string","enum":["0","1"]},
            "enabled":{"type":"boolean","default":false},
            "name":{"type":"string","example":"a sample"},
            "unknown":{}
        }}}}
    }}}}}});
    let spec = normalize_spec(&document).unwrap();
    assert_eq!(
        spec.operations[0].response.as_ref().unwrap().schema.shape(),
        json!({"state":"integer","label":"string","enabled":"boolean","name":"string","unknown":"any"})
    );
}

#[test]
fn openapi_response_fixtures_cover_json_yaml_and_swagger() {
    use crate::domain::functions::spec_output::operation_json;
    for (document, expected) in [
        (
            serde_json::from_str::<Value>(include_str!(
                "../../../tests/fixtures/openapi/responses.json"
            ))
            .unwrap(),
            json!({"content":[{"id":"integer","name":"string","parent":{}}],"page":"integer"}),
        ),
        (
            serde_saphyr::from_slice_with_options::<Value>(
                include_bytes!("../../../tests/fixtures/openapi/responses.yaml"),
                serde_saphyr::Options::default(),
            )
            .unwrap(),
            json!([{"id":"integer","name":"string"}]),
        ),
        (
            serde_json::from_str::<Value>(include_str!(
                "../../../tests/fixtures/openapi/responses-swagger.json"
            ))
            .unwrap(),
            json!([{"id":"integer","name":"string"}]),
        ),
    ] {
        let spec = normalize_spec(&document).unwrap();
        assert_eq!(
            operation_json("pets", &spec.operations[0])["response"]["shape"],
            expected
        );
    }
}

#[test]
fn swagger_response_media_type_falls_back_to_the_document_produces() {
    let document = json!({
        "swagger": "2.0",
        "info": {"title": "T", "version": "1"},
        "produces": ["application/xml"],
        "paths": {"/a": {
            "get": {"operationId": "a", "responses": {"200": {
                "description": "ok", "schema": {"type": "string"}
            }}},
            "put": {"operationId": "b", "produces": ["text/plain"], "responses": {"200": {
                "description": "ok", "schema": {"type": "string"}
            }}}
        }}
    });
    let spec = normalize_spec(&document).unwrap();
    let media = |id: &str| {
        let operation = spec.operations.iter().find(|o| o.id == id).unwrap();
        operation.response.as_ref().unwrap().content_type.clone()
    };
    assert_eq!(media("a"), "application/xml");
    assert_eq!(media("b"), "text/plain");
}

#[test]
fn every_reduction_leaves_a_limitation_where_it_happened() {
    use crate::domain::types::api_spec::SchemaLimitation;
    let mut nested = json!({"type": "string"});
    for level in 0..(MAX_SCHEMA_DEPTH + 1) {
        nested = json!({"type": "object", "properties": {format!("l{level}"): nested}});
    }
    let document = json!({
        "openapi": "3.1.0",
        "info": {"title": "T", "version": "1"},
        "paths": {"/x": {"post": {
            "operationId": "x",
            "requestBody": {"content": {
                "application/xml": {"schema": {"type": "string"}},
                "application/json": {"schema": {"$ref": "#/components/schemas/Node"}},
                "text/plain": {"schema": {"type": "string"}}
            }},
            "responses": {"200": {"description": "ok", "content": {
                "application/json": {"schema": {"anyOf": [{"type": "string"}, {"type": "integer"}]}},
                "text/csv": {"schema": {"type": "string"}}
            }}}
        }}},
        "components": {"schemas": {
            "Node": {"type": ["object", "null"], "properties": {
                "next": {"$ref": "#/components/schemas/Node"},
                "pick": {"oneOf": [{"type": "string"}, {"type": "integer"}, {"type": "boolean"}]},
                "deep": nested
            }}
        }}
    });
    let spec = normalize_spec(&document).unwrap();
    let operation = &spec.operations[0];
    let body = operation.request_body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/json");
    assert_eq!(body.other_content_types, ["application/xml", "text/plain"]);
    assert_eq!(
        body.schema.limitations,
        [SchemaLimitation::OtherTypes {
            types: vec!["null".into()]
        }]
    );
    let property = |name: &str| {
        &body
            .schema
            .properties
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .schema
    };
    assert_eq!(
        property("next").limitations,
        [SchemaLimitation::Cycle {
            reference: "#/components/schemas/Node".into()
        }]
    );
    assert_eq!(
        property("pick").limitations,
        [SchemaLimitation::FirstAlternativeOnly {
            keyword: "oneOf",
            alternatives: 3
        }]
    );
    let mut deepest = property("deep");
    while let Some(next) = deepest.properties.first() {
        deepest = &next.schema;
    }
    assert_eq!(deepest.limitations, [SchemaLimitation::DepthBound]);
    let response = operation.response.as_ref().unwrap();
    assert_eq!(response.other_content_types, ["text/csv"]);
    assert_eq!(
        response.schema.limitations,
        [SchemaLimitation::FirstAlternativeOnly {
            keyword: "anyOf",
            alternatives: 2
        }]
    );
    let nullable = normalize_spec(&json!({
        "openapi": "3.0.3",
        "info": {"title": "T", "version": "1"},
        "paths": {"/z": {"get": {"operationId": "z", "responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"type": "string", "nullable": true}}}}}}}}
    }))
    .unwrap();
    assert_eq!(
        nullable.operations[0]
            .response
            .as_ref()
            .unwrap()
            .schema
            .limitations,
        [SchemaLimitation::OtherTypes {
            types: vec!["null".into()]
        }],
        "3.0 nullable is the same fact as a 3.1 null type"
    );
    let single = normalize_spec(&json!({
        "openapi": "3.0.3",
        "info": {"title": "T", "version": "1"},
        "paths": {"/y": {"get": {"operationId": "y", "responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"oneOf": [{"type": "string"}]}}}}}}}}
    }))
    .unwrap();
    let response = single.operations[0].response.as_ref().unwrap();
    assert!(response.other_content_types.is_empty());
    assert!(
        response.schema.limitations.is_empty(),
        "one alternative drops nothing"
    );
}

/// The Manual REST contract: `openapi` and `paths` are all a
/// hand-written description needs. What it leaves out is a default, not a
/// warning: no `info` is the title `API`, no `responses` is no response
/// shape, no `operationId` is `METHOD /path`, and a `{name}` of the path
/// template that no parameter declares is a required string path parameter.
#[test]
fn a_minimal_document_needs_only_openapi_and_paths() {
    let spec = normalize_spec(&json!({
        "openapi": "3.1.0",
        "paths": {
            "/api/v2/issues/{issueIdOrKey}/comments/{commentId}": {"get": {
                "operationId": "comments/get",
                "parameters": [{"name": "commentId", "in": "path", "schema": {"type": "integer"}}]
            }},
            "/api/v2/users/myself": {"get": {}}
        }
    }))
    .unwrap();
    assert_eq!(spec.title, "API");
    assert!(spec.warnings.is_empty(), "{:?}", spec.warnings);
    let comment = &spec.operations[0];
    assert_eq!(comment.id, "comments/get");
    assert_eq!(comment.response, None);
    let parameters: Vec<(&str, ParameterLocation, bool, &str)> = comment
        .parameters
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.location,
                p.required,
                p.schema.type_name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        parameters,
        [
            ("commentId", ParameterLocation::Path, true, "integer"),
            ("issueIdOrKey", ParameterLocation::Path, true, "string"),
        ],
        "a declared parameter keeps its schema; an undeclared one is a string"
    );
    let myself = &spec.operations[1];
    assert_eq!(myself.id, "GET /api/v2/users/myself");
    assert!(!myself.has_operation_id);
    assert!(myself.parameters.is_empty());
}
