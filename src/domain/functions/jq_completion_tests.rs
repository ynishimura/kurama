//! Regression tests for jq completion and input behavior.

use super::*;
use serde_json::json;

fn shape() -> JsonShape {
    JsonShape::from_value(&json!({
        "content": [{"accountId": 7, "accountName": "日本語", "enabled": false, "nested": {"id": 2}}],
        "count": 1,
        "weird.key": {"with space": "line\nnext"},
        "quote\"key": 1,
        "back\\slash": 2
    }))
}

fn values(text: &str) -> Vec<String> {
    complete(&shape(), text, text.len())
        .candidates
        .into_iter()
        .map(|candidate| candidate.value)
        .collect()
}

#[test]
fn jq_completion_traverses_fields_arrays_and_prefixes() {
    assert_eq!(values(".co"), [".content", ".count"]);
    assert_eq!(values(".content"), [".content[]", ".content[0]"]);
    assert_eq!(values(".content[].accountN"), [".content[].accountName"]);
    assert_eq!(values(".content[0].nested."), [".content[0].nested.id"]);
    assert!(values(".missing.").is_empty());
    assert!(values(".count.").is_empty());
    assert_eq!(values(".content[] | select(.ena"), [".enabled"]);
    assert_eq!(values(".content | map(.accountN"), [".accountName"]);
}

/// A pipe or a call inside an index is jq nobody's path can follow: no
/// context, so no candidates, whatever the depth of the brackets.
#[test]
fn jq_completion_offers_nothing_after_a_pipe_or_a_call_inside_an_index() {
    assert!(context(&shape(), ".content[.count | ").is_none());
    assert!(context(&shape(), ".content[map(").is_none());
    assert!(context(&shape(), ".content[0][.x | ").is_none());
    assert!(context(&shape(), ".content[] | ").is_some());
}

#[test]
fn jq_completion_describes_actual_types_and_escaped_samples() {
    let completion = complete(&shape(), ".content[0].", 12);
    let name = completion
        .candidates
        .iter()
        .find(|c| c.value.ends_with("accountName"))
        .unwrap();
    assert_eq!(name.description, "string  \"日本語\"");
    let completion = complete(&shape(), ".", 1);
    assert_eq!(
        completion
            .candidates
            .iter()
            .find(|c| c.value == ".content")
            .unwrap()
            .description,
        "array[1]"
    );
    assert!(
        completion
            .candidates
            .iter()
            .find(|c| c.value == ".content")
            .unwrap()
            .continues
    );
    assert_eq!(
        values(".[\"weird.key\"]."),
        [".[\"weird.key\"][\"with space\"]"]
    );
    let candidate = complete(&shape(), ".[\"weird.key\"].", 15)
        .candidates
        .remove(0);
    assert!(!candidate.description.contains('\n'));
    assert!(candidate.description.contains("\\n"));
}

#[test]
fn jq_completion_replaces_only_the_prefix_and_keeps_unicode_suffix() {
    let text = ".content[] | select(.ena) # 日本語";
    let cursor = text.find(") #").unwrap();
    let candidate = complete(&shape(), text, cursor).candidates.remove(0);
    let (text, cursor) = apply(text, cursor, &candidate);
    assert_eq!(text, ".content[] | select(.enabled) # 日本語");
    assert_eq!(&text[cursor..], ") # 日本語");
    let candidate = complete(&shape(), "ma", 2).candidates.remove(0);
    let (text, cursor) = apply("ma", 2, &candidate);
    assert_eq!(text, "map()");
    assert_eq!(&text[cursor..], ")");
}

#[test]
fn jq_completion_offers_builtins_after_a_pipe_and_handles_empty_shapes() {
    for text in ["", ".content | ", ".content | le"] {
        let values = values(text);
        assert!(values.iter().any(|value| value == "length"));
    }
    let array = JsonShape::from_value(&json!([{ "id": 1 }, { "id": 2 }]));
    assert_eq!(
        complete(&array, ".", 1)
            .candidates
            .iter()
            .map(|c| c.value.as_str())
            .collect::<Vec<_>>(),
        [".[]", ".[0]"]
    );
    assert_eq!(complete(&array, ".[0].", 5).candidates[0].value, ".[0].id");
    assert_eq!(complete(&array, ".[1].", 5).candidates[0].value, ".[1].id");
    for value in [json!(null), json!(7), json!({}), json!([])] {
        let shape = JsonShape::from_value(&value);
        assert!(complete(&shape, ".missing.", 9).candidates.is_empty());
    }
    assert!(complete(&JsonShape::Unknown, ".", 1).candidates.is_empty());
    assert!(values(".content | {x: .accountId} | .").is_empty());
}

#[test]
fn jq_examples_follow_response_structure_and_always_have_a_small_menu() {
    let menu = examples(&shape());
    let filters: Vec<_> = menu.iter().map(|example| example.filter.as_str()).collect();
    assert!((6..=10).contains(&filters.len()));
    assert!(filters.contains(&".content | length"));
    assert!(filters.contains(&".content[] | select(.enabled)"));
    assert!(filters.contains(&".content | map(.accountName) | sort"));
    assert!(filters.contains(&".content[0] | keys"));
    for value in [json!([{"id": 1}]), json!({"id": 2}), json!(null), json!([])] {
        assert!((6..=10).contains(&examples(&JsonShape::from_value(&value)).len()));
    }
}

#[test]
fn jq_completion_preserves_a_matching_suffix_once_and_finds_quoted_keys() {
    let text = ".content[].accountName | type";
    let cursor = text.find("Name").unwrap() + 2;
    let candidate = complete(&shape(), text, cursor).candidates.remove(0);
    let (actual, cursor) = apply(text, cursor, &candidate);
    assert_eq!(actual, text);
    assert_eq!(&actual[cursor..], " | type");
    assert_eq!(values(".wei"), [".[\"weird.key\"]"]);
    assert_eq!(
        values(".[\"weird.key\"].wi"),
        [".[\"weird.key\"][\"with space\"]"]
    );
    assert_eq!(values(".quo"), [".[\"quote\\\"key\"]"]);
    assert_eq!(values(".bac"), [".[\"back\\\\slash\"]"]);
    assert_eq!(
        values(".\"weird.key\"."),
        [".\"weird.key\"[\"with space\"]"]
    );
    assert_eq!(values(".[\"quote\\\""), [".[\"quote\\\"key\"]"]);
    assert_eq!(values(".[\"back\\\\"), [".[\"back\\\\slash\"]"]);
    assert!(values(".[\"unterminated").is_empty());
    assert_eq!(values(".[\"weird.key\""), [".[\"weird.key\"]"]);
}

#[test]
fn jq_completion_tracks_known_nested_inputs_and_declines_unknown_transforms() {
    assert_eq!(values(".content | map(select(.ena"), [".enabled"]);
    assert_eq!(values(".content[] | (.nested | ."), [".id"]);
    assert!(values(".content | map(.nested) | .").is_empty());
    assert!(values(".content | unknown(.").is_empty());
    let shape = JsonShape::from_value(&json!([{"archived": true}]));
    let example = examples(&shape)
        .into_iter()
        .find(|example| example.filter.contains("select"))
        .unwrap();
    assert!(example.description.contains(".archived is true"));
}

#[test]
fn jq_completion_keeps_outer_delimiters_and_declines_array_constructor_pipes() {
    let candidate = complete(&shape(), "select(ma)", 9).candidates.remove(0);
    assert_eq!(
        apply("select(ma)", 9, &candidate),
        ("select(map())".into(), 11)
    );
    let shape = JsonShape::from_value(&json!([1]));
    let candidate = complete(&shape, "[.]", 2).candidates.remove(0);
    assert_eq!(apply("[.]", 2, &candidate).0, "[.[]]");
    assert!(values("[.content[] | .").is_empty());
}

#[test]
fn jq_completion_preserves_existing_function_arguments() {
    let text = "map(.accountName)";
    let candidate = complete(&shape(), text, 2).candidates.remove(0);
    assert_eq!(apply(text, 2, &candidate), (text.into(), 4));
    let array = JsonShape::from_value(&json!([{"id": 1}]));
    assert!(complete(&array, "[map(.", 6).candidates.is_empty());
}

#[test]
fn jq_completion_replaces_the_rest_of_the_current_name_when_choosing_another_field() {
    let text = ".content[].accountId | type";
    let cursor = text.find("Id").unwrap();
    let candidate = complete(&shape(), text, cursor)
        .candidates
        .into_iter()
        .find(|c| c.value.ends_with("accountName"))
        .unwrap();
    assert_eq!(
        apply(text, cursor, &candidate).0,
        ".content[].accountName | type"
    );
    let text = "sort_by(.id)";
    let candidate = complete(&shape(), text, 1)
        .candidates
        .into_iter()
        .find(|c| c.value == "select()")
        .unwrap();
    assert_eq!(apply(text, 1, &candidate), ("select(.id)".into(), 7));
}

#[test]
fn jq_completion_replaces_a_quoted_key_or_array_index_without_losing_following_paths() {
    let text = ".[\"weird.key\"][\"with space\"] | type";
    let cursor = text.find(".key").unwrap();
    let candidate = complete(&shape(), text, cursor).candidates.remove(0);
    assert_eq!(apply(text, cursor, &candidate).0, text);
    let text = ".content[0].accountName";
    let cursor = text.find('0').unwrap();
    let candidate = complete(&shape(), text, cursor).candidates.remove(0);
    assert_eq!(apply(text, cursor, &candidate).0, ".content[].accountName");
}

#[test]
fn jq_completion_does_not_expand_children_before_an_existing_path() {
    for (text, at, expected) in [
        (".content[0].accountName", "[0]", Some(".content")),
        (".content[].nested.id", ".id", Some(".content[].nested")),
        (".content[].accountName", ".account", None),
    ] {
        let cursor = text.find(at).unwrap();
        let completion = complete(&shape(), text, cursor);
        match expected {
            Some(expected) => {
                assert_eq!(completion.candidates.len(), 1);
                assert_eq!(completion.candidates[0].value, expected);
                assert_eq!(apply(text, cursor, &completion.candidates[0]).0, text);
            }
            None => assert!(completion.candidates.is_empty()),
        }
    }
}

#[test]
fn field_path_uses_a_dot_for_an_identifier_and_brackets_otherwise() {
    assert_eq!(field_path(".", "items"), ".items");
    assert_eq!(field_path(".items[3]", "name"), ".items[3].name");
    assert_eq!(field_path(".", "a-b"), ".[\"a-b\"]");
    assert_eq!(field_path(".items", "with space"), ".items[\"with space\"]");
    assert_eq!(field_path(".", "1st"), ".[\"1st\"]");
}
