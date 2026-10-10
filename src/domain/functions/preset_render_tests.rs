//! Tests of the preset catalog and of `plan_preset`: the catalog rules, reuse and refusal of an existing auth, `--as`, missing inputs and the TOML.

use super::*;
use crate::domain::types::preset::{PRESETS, PresetSpec, find_preset};
use crate::domain::types::{GrantType, OAuthClientConfig, TokenSourceConfig};

const PATH: &str = "/home/me/.config/kurama/config.toml";

fn existing<'a>(apis: &'a [String], auth: Option<&'a AuthSource>) -> Existing<'a> {
    Existing {
        apis,
        auth,
        vault: Some("Agent"),
        config_path: PATH,
    }
}

fn request(api: Option<&str>, inputs: &[(&str, &str)]) -> PresetRequest {
    PresetRequest {
        api_name: api.map(str::to_owned),
        auth_name: None,
        inputs: inputs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

/// Every input a preset declares, with a value of the right shape.
fn full_request(preset: &Preset) -> PresetRequest {
    let inputs: Vec<(&str, String)> = preset
        .inputs
        .iter()
        .map(|input| {
            let value = match input.secret_field {
                Some(field) => format!("op://Agent/{}/{field}", preset.auth.item()),
                None => format!("{}-value", input.key),
            };
            (input.key, value)
        })
        .collect();
    PresetRequest {
        api_name: None,
        auth_name: None,
        inputs: inputs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    }
}

fn planned(preset: &str, request: &PresetRequest, existing: &Existing) -> PresetPlan {
    plan_preset(
        find_preset(preset).unwrap(),
        request,
        existing,
        "2026-09-26",
    )
    .unwrap()
}

fn google(scopes: &[&str], client_id: &str) -> AuthSource {
    AuthSource::OAuth(OAuthClientConfig {
        name: "google".into(),
        grant_type: GrantType::AuthorizationCode,
        endpoints: EndpointSource::Issuer("https://accounts.google.com".into()),
        client_id: client_id.into(),
        client_secret: Some(SecretRef::parse("op://Agent/kurama-google/client_secret").unwrap()),
        scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        env_var: "KURAMA_TOKEN".into(),
        redirect_port: Some(8080),
    })
}

#[test]
fn preset_ids_are_unique_and_found() {
    for preset in PRESETS {
        assert_eq!(
            PRESETS.iter().filter(|other| other.id == preset.id).count(),
            1,
            "{}",
            preset.id
        );
        assert!(std::ptr::eq(find_preset(preset.id).unwrap(), preset));
    }
    assert!(find_preset("nope").is_none());
}

#[test]
fn every_url_is_https() {
    for preset in PRESETS {
        let mut urls = vec![preset.docs_url, preset.api.base_url];
        urls.extend(preset.api.spec.and_then(PresetSpec::url));
        urls.extend(auth_templates(&preset.auth.kind).filter(|value| value.contains("://")));
        for step in preset.setup.iter().chain(preset.auth.setup) {
            urls.extend(step.split_whitespace().filter(|word| word.contains("://")));
        }
        for url in urls {
            // `op://` in a setup step is a reference, not a URL.
            if url.starts_with("op://") {
                continue;
            }
            assert!(url.starts_with("https://"), "{}: {url}", preset.id);
        }
    }
}

#[test]
fn template_variables_and_declared_inputs_are_one_to_one() {
    for preset in PRESETS {
        let mut named: Vec<String> = auth_templates(&preset.auth.kind)
            .chain([preset.api.base_url])
            .chain(preset.api.spec.map(PresetSpec::value))
            .flat_map(template_variables)
            .collect();
        named.sort();
        named.dedup();
        let mut declared: Vec<String> = input_keys(preset).into_iter().map(str::to_owned).collect();
        declared.sort();
        assert_eq!(named, declared, "{}", preset.id);
    }
}

#[test]
fn a_new_api_references_the_auth_the_same_preset_writes() {
    for preset in PRESETS {
        let apis = [];
        let text = planned(preset.id, &full_request(preset), &existing(&apis, None))
            .fragment
            .unwrap();
        assert!(text.contains(&format!("[auth.{}]\n", preset.auth.name)));
        assert!(text.contains(&format!("[api.{}]\n", preset.api.name)));
        assert!(text.contains(&format!("auth = \"{}\"\n", preset.auth.name)));
    }
}

#[test]
fn every_setup_variable_is_resolved_and_items_are_referenceable() {
    for preset in PRESETS {
        let item = preset.auth.item();
        assert!(
            !item.contains(['(', ')', ' ']),
            "{}: an op:// reference cannot name {item:?}",
            preset.id
        );
        let apis = [];
        let empty = PresetRequest::default();
        for request in [&empty, &full_request(preset)] {
            let plan = planned(preset.id, request, &existing(&apis, None));
            for step in &plan.setup {
                assert!(template_variables(step).is_empty(), "{}: {step}", preset.id);
                for reference in step
                    .split_whitespace()
                    .filter_map(|word| word.split_once("op://").map(|(_, rest)| rest))
                {
                    assert!(!reference.contains(['(', ')']), "{reference}");
                }
            }
        }
    }
}

#[test]
fn missing_inputs_are_listed_and_the_setup_is_still_planned() {
    let apis = [];
    let plan = planned(
        "google-sheets",
        &request(None, &[("client_id", "x")]),
        &existing(&apis, None),
    );
    assert_eq!(
        plan.fragment,
        Err(PresetError::MissingInputs {
            id: "google-sheets".into(),
            keys: vec!["client_secret".into()]
        })
    );
    assert!(plan.setup.iter().any(|step| step.contains(
        "kurama preset setup google-sheets --set client_id=x --set client_secret=op://Agent/kurama-google/client_secret"
    )));
    // What is left after the append: the login and a first call.
    let next = &plan.setup[plan.after_append..];
    assert!(plan.setup[plan.after_append - 1].contains("kurama preset setup google-sheets"));
    assert_eq!(next.len(), 2, "{next:?}");
    assert!(next[0].ends_with("kurama login google"), "{next:?}");
    assert!(next[1].contains("kurama api google-sheets "), "{next:?}");
}

/// A secret input that is not a reference is refused by its key before the
/// file's auth is compared with it, so a compatible auth is never called
/// the problem; the value is echoed nowhere.
#[test]
fn a_literal_secret_is_refused_by_key_before_the_auth_is_compared() {
    let apis = [];
    let linear = AuthSource::Token(TokenSourceConfig {
        name: "linear".into(),
        token: SecretRef::parse("op://Agent/kurama-linear/credential").unwrap(),
        placement: crate::domain::types::TokenPlacement::Header {
            name: DEFAULT_TOKEN_HEADER.into(),
            format: "{token}".into(),
        },
        env_var: "LINEAR_API_KEY".into(),
    });
    for auth in [None, Some(&linear)] {
        let error = plan_preset(
            find_preset("linear").unwrap(),
            &request(Some("l2"), &[("secret", "lin_api_literal")]),
            &existing(&apis, auth),
            "d",
        )
        .unwrap_err();
        assert_eq!(
            error,
            PresetError::LiteralSecret {
                keys: vec!["secret".into()]
            }
        );
        let text = format!("{error} {}", error.hint());
        assert!(!text.contains("lin_api_literal"), "{text}");
        assert!(
            text.starts_with("--set secret must be a secret reference"),
            "{text}"
        );
    }
    // A client_id is not a secret: any value is taken.
    let plan = planned(
        "google-sheets",
        &request(
            None,
            &[("client_id", "plain"), ("client_secret", "op://A/b/c")],
        ),
        &existing(&apis, None),
    );
    assert!(plan.fragment.is_ok());
}

#[test]
fn an_input_the_preset_does_not_declare_is_refused() {
    let apis = [];
    let error = plan_preset(
        find_preset("github").unwrap(),
        &request(None, &[("client_id", "x")]),
        &existing(&apis, None),
        "d",
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "preset github takes no input client_id; it takes secret"
    );
}

#[test]
fn as_renames_only_the_api() {
    let apis = ["google-sheets".to_owned()];
    let preset = find_preset("google-sheets").unwrap();
    let mut asked = full_request(preset);
    asked.api_name = Some("sheets-work".into());
    let plan = planned("google-sheets", &asked, &existing(&apis, None));
    let text = plan.fragment.unwrap();
    assert!(text.contains("[auth.google]\n"));
    assert!(text.contains("[api.sheets-work]\nd"));
    assert!(text.contains("auth = \"google\"\n"));
    assert!(!text.contains("auth.sheets-work"));
    assert_eq!(
        (plan.api.as_str(), plan.auth.as_str()),
        ("sheets-work", "google")
    );
    assert!(
        plan.setup
            .iter()
            .any(|step| step.contains("--as sheets-work"))
    );
    assert!(
        plan.setup
            .iter()
            .any(|step| step.ends_with("kurama login google"))
    );
    assert!(
        plan.setup
            .iter()
            .any(|step| step.contains("kurama api sheets-work sheets.spreadsheets.get"))
    );
}

/// `--auth-as` names the auth the preset writes, the API's reference to it
/// and every step that names it; the env_var stays the preset's.
#[test]
fn auth_as_renames_the_auth_and_every_reference_to_it() {
    let apis = [];
    let preset = find_preset("google-docs").unwrap();
    let mut asked = full_request(preset);
    asked.api_name = Some("docs-work".into());
    asked.auth_name = Some("google-work".into());
    let plan = planned("google-docs", &asked, &existing(&apis, None));
    assert_eq!(
        (plan.api.as_str(), plan.auth.as_str()),
        ("docs-work", "google-work")
    );
    let text = plan.fragment.unwrap();
    assert!(text.contains("[auth.google-work]\n"), "{text}");
    assert!(text.contains("[api.docs-work]\n"), "{text}");
    assert!(text.contains("auth = \"google-work\"\n"), "{text}");
    assert!(!text.contains("[auth.google]"), "{text}");
    assert!(
        plan.setup.iter().any(|step| step
            .contains("kurama preset setup google-docs --as docs-work --auth-as google-work ")),
        "{:#?}",
        plan.setup
    );
    assert!(
        plan.setup
            .iter()
            .any(|step| step.ends_with("kurama login google-work")),
        "{:#?}",
        plan.setup
    );
    // The token keeps the preset's env_var.
    let mut asked = full_request(find_preset("github").unwrap());
    asked.auth_name = Some("gh-pat".into());
    let text = planned("github", &asked, &existing(&apis, None))
        .fragment
        .unwrap();
    assert!(text.contains("[auth.gh-pat]\n"), "{text}");
    assert!(text.contains("env_var = \"GITHUB_TOKEN\"\n"), "{text}");
    assert!(text.contains("[api.github]\n"), "{text}");
    assert!(text.contains("auth = \"gh-pat\"\n"), "{text}");
}

/// A compatible auth under the `--auth-as` name is reused, and the scope
/// step and warning name it.
#[test]
fn auth_as_reuses_a_compatible_auth_of_that_name() {
    let apis = [];
    let held = google(
        &["https://www.googleapis.com/auth/spreadsheets.readonly"],
        "id-1",
    );
    let asked = PresetRequest {
        auth_name: Some("google-work".into()),
        ..PresetRequest::default()
    };
    let plan = planned("google-docs", &asked, &existing(&apis, Some(&held)));
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    let text = plan.fragment.unwrap();
    assert!(!text.contains("[auth."), "{text}");
    assert!(text.contains("auth = \"google-work\"\n"), "{text}");
    assert!(
        plan.warnings[0].contains("[auth.google-work] lacks")
            && plan.warnings[0].contains("kurama config set auth.google-work.scopes")
            && plan.warnings[0].contains("kurama login --force google-work"),
        "{:?}",
        plan.warnings
    );
    // An incompatible one of that name is refused under that name.
    let token = AuthSource::Token(TokenSourceConfig {
        name: "google-work".into(),
        token: SecretRef::parse("op://Agent/gh/credential").unwrap(),
        placement: crate::domain::types::TokenPlacement::Header {
            name: DEFAULT_TOKEN_HEADER.into(),
            format: DEFAULT_TOKEN_FORMAT.into(),
        },
        env_var: "GOOGLE_TOKEN".into(),
    });
    let error = plan_preset(
        find_preset("google-docs").unwrap(),
        &asked,
        &existing(&apis, Some(&token)),
        "d",
    )
    .unwrap_err();
    assert!(
        error.to_string().starts_with("[auth.google-work] in "),
        "{error}"
    );
}

#[test]
fn an_auth_named_like_an_aws_profile_hints_at_auth_as() {
    let hint = PresetError::AuthNamedLikeAwsProfile("dev".into()).hint();
    assert!(
        hint.starts_with("give the auth another name with --auth-as <name>"),
        "{hint}"
    );
}

#[test]
fn a_taken_api_name_is_refused_with_as_in_the_hint() {
    let apis = ["github".to_owned()];
    let error = plan_preset(
        find_preset("github").unwrap(),
        &PresetRequest::default(),
        &existing(&apis, None),
        "d",
    )
    .unwrap_err();
    assert!(matches!(error, PresetError::ApiTaken { .. }));
    assert!(error.hint().contains("--as"));
}

#[test]
fn a_compatible_auth_is_reused_and_only_its_missing_scopes_warn() {
    let apis = [];
    let held = google(
        &["https://www.googleapis.com/auth/spreadsheets.readonly"],
        "id-1",
    );
    // Reusing needs neither client input.
    let plan = planned(
        "google-docs",
        &PresetRequest::default(),
        &existing(&apis, Some(&held)),
    );
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    let text = plan.fragment.unwrap();
    assert!(!text.contains("[auth.google]"));
    assert!(text.contains("[api.google-docs]\n"));
    assert_eq!(plan.warnings.len(), 1);
    assert!(plan.warnings[0].contains("documents.readonly"));
    assert!(plan.warnings[0].contains("kurama login --force google"));
    assert!(
        plan.setup
            .iter()
            .any(|step| step.ends_with("kurama login --force google"))
    );
    assert!(
        !plan
            .setup
            .iter()
            .any(|step| step.contains("op item create"))
    );
    // The same scopes: nothing to warn about, nothing to log in again.
    let plan = planned(
        "google-sheets",
        &request(None, &[("client_id", "id-1")]),
        &existing(&apis, Some(&held)),
    );
    assert!(plan.warnings.is_empty());
    assert!(!plan.setup.iter().any(|step| step.contains("kurama login")));
}

#[test]
fn an_incompatible_auth_is_refused_with_its_reason() {
    let apis = [];
    let refuse = |preset: &str, request: &PresetRequest, held: &AuthSource| {
        let error = plan_preset(
            find_preset(preset).unwrap(),
            request,
            &existing(&apis, Some(held)),
            "d",
        )
        .unwrap_err();
        assert!(!error.hint().contains("--as "), "{error}");
        assert!(
            error
                .hint()
                .starts_with("give the auth another name with --auth-as <name>"),
            "{error}"
        );
        error.to_string()
    };
    let held = google(&[], "id-1");
    assert!(
        refuse(
            "google-docs",
            &request(None, &[("client_id", "id-2")]),
            &held
        )
        .contains("the client_id given with --set is not the one it holds")
    );
    assert!(
        refuse(
            "google-docs",
            &request(None, &[("client_secret", "op://Agent/other/client_secret")]),
            &held
        )
        .contains("client_secret")
    );
    // A GitHub token auth is not the OAuth app the device flow needs.
    let token = AuthSource::Token(TokenSourceConfig {
        name: "github".into(),
        token: SecretRef::parse("op://Agent/gh/credential").unwrap(),
        placement: crate::domain::types::TokenPlacement::Header {
            name: DEFAULT_TOKEN_HEADER.into(),
            format: DEFAULT_TOKEN_FORMAT.into(),
        },
        env_var: "GITHUB_TOKEN".into(),
    });
    assert!(
        refuse("github-oauth", &PresetRequest::default(), &token)
            .contains("it is kind = \"token\", the preset oauth device_code")
    );
    // Linear sends its key without Bearer: the default format is not it.
    let linear = AuthSource::Token(TokenSourceConfig {
        name: "linear".into(),
        ..match token.clone() {
            AuthSource::Token(source) => source,
            AuthSource::OAuth(_) | AuthSource::Secrets(_) => unreachable!(),
        }
    });
    assert!(refuse("linear", &PresetRequest::default(), &linear).contains("Bearer {token}"));
    // The same token reference is reused; another one is refused.
    let plan = planned(
        "github",
        &request(None, &[("secret", "op://Agent/gh/credential")]),
        &existing(&apis, Some(&token)),
    );
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    assert!(
        refuse(
            "github",
            &request(None, &[("secret", "op://Agent/x/credential")]),
            &token
        )
        .contains("secret")
    );
    // Another issuer is another provider.
    let other = match google(&[], "id-1") {
        AuthSource::OAuth(mut client) => {
            client.endpoints = EndpointSource::Issuer("https://login.example.com".into());
            AuthSource::OAuth(client)
        }
        AuthSource::Token(_) | AuthSource::Secrets(_) => unreachable!(),
    };
    assert!(refuse("google-drive", &PresetRequest::default(), &other).contains("issuer"));
}

/// A token auth is reused only where it puts the credential where the
/// preset would: the same place, and for Basic the username the inputs give.
#[test]
fn a_token_auth_is_reused_only_with_the_same_placement() {
    let jira = |username: &str| {
        AuthSource::Token(TokenSourceConfig {
            name: "jira".into(),
            token: SecretRef::parse("op://Agent/kurama-jira/credential").unwrap(),
            placement: crate::domain::types::TokenPlacement::Basic {
                username: username.into(),
            },
            env_var: "JIRA_API_TOKEN".into(),
        })
    };
    let apis = [];
    let refuse = |preset: &str, request: &PresetRequest, held: &AuthSource| {
        plan_preset(
            find_preset(preset).unwrap(),
            request,
            &existing(&apis, Some(held)),
            "d",
        )
        .unwrap_err()
        .to_string()
    };
    let site = &[("site", "example"), ("email", "me@example.com")];
    let plan = planned(
        "jira",
        &request(None, site),
        &existing(&apis, Some(&jira("me@example.com"))),
    );
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    // Without the email the held username is not compared.
    let plan = planned(
        "jira",
        &request(None, &[("site", "example")]),
        &existing(&apis, Some(&jira("other@example.com"))),
    );
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    assert!(
        refuse("jira", &request(None, site), &jira("other@example.com"))
            .contains("the username given with --set is not the one it holds")
    );
    // A Basic auth is not the query parameter Backlog needs.
    let backlog = AuthSource::Token(TokenSourceConfig {
        name: "backlog".into(),
        ..match jira("me@example.com") {
            AuthSource::Token(source) => source,
            AuthSource::OAuth(_) | AuthSource::Secrets(_) => unreachable!(),
        }
    });
    assert!(
        refuse("backlog", &PresetRequest::default(), &backlog)
            .contains("it sends the token as basic, the preset as query apiKey")
    );
}

#[test]
fn values_are_quoted_as_toml_strings() {
    assert_eq!(quote("a\"b\\c\nd"), "\"a\\\"b\\\\c\\nd\"");
    assert_eq!(quote("\u{1}"), "\"\\u0001\"");
}

#[test]
fn template_variables_skip_json_braces() {
    assert_eq!(
        template_variables("https://{site}/x/{a_b}"),
        ["site", "a_b"]
    );
    assert!(template_variables("-d '{\"query\":\"{ viewer { id } }\"}'").is_empty());
}

#[test]
fn the_provenance_comment_opens_the_fragment() {
    let apis = [];
    let preset = find_preset("github").unwrap();
    let text = planned("github", &full_request(preset), &existing(&apis, None))
        .fragment
        .unwrap();
    assert!(text.starts_with(&format!(
        "# kurama preset: github (kurama {}, 2026-09-26)\n# setup: https://github.com/settings/personal-access-tokens/new\n[auth.github]\n",
        env!("CARGO_PKG_VERSION")
    )));
}

/// Scopes a reused auth lacks are a numbered step before the login that
/// needs them, so following the steps in order asks for the new scope.
#[test]
fn a_scope_shortfall_is_a_step_before_the_login() {
    let apis = [];
    let held = google(
        &["https://www.googleapis.com/auth/spreadsheets.readonly"],
        "id-1",
    );
    let plan = planned(
        "google-docs",
        &PresetRequest::default(),
        &existing(&apis, Some(&held)),
    );
    // The command writes the whole list: what the auth holds, then what it
    // lacks.
    let command = "kurama config set auth.google.scopes '[\"https://www.googleapis.com/auth/spreadsheets.readonly\",\"https://www.googleapis.com/auth/documents.readonly\"]'";
    let add = plan
        .setup
        .iter()
        .position(|step| step.contains("[auth.google]") && step.ends_with(&format!("\n{command}")))
        .expect("a step adds the scope");
    assert!(plan.warnings[0].contains(command), "{:?}", plan.warnings);
    let login = plan
        .setup
        .iter()
        .position(|step| step.ends_with("kurama login --force google"))
        .unwrap();
    assert!(add < login, "{:#?}", plan.setup);
}

/// A client credentials grant needs no person: a new auth goes from the
/// append straight to the first call, which gets the token; a reused one
/// whose scopes grew still logs in again to replace the stored token.
#[test]
fn a_client_credentials_auth_logs_in_only_to_replace_a_token() {
    let apis = [];
    let plan = planned(
        "zendesk",
        &full_request(find_preset("zendesk").unwrap()),
        &existing(&apis, None),
    );
    let next = &plan.setup[plan.after_append..];
    assert_eq!(next.len(), 1, "{next:?}");
    assert!(next[0].contains("kurama api zendesk "), "{next:?}");

    let held = AuthSource::OAuth(OAuthClientConfig {
        name: "zendesk".into(),
        grant_type: GrantType::ClientCredentials,
        endpoints: EndpointSource::Explicit(crate::domain::types::OAuthEndpoints {
            auth_url: None,
            token_url: "https://example.zendesk.com/oauth/tokens".into(),
            device_auth_url: None,
        }),
        client_id: "id-1".into(),
        client_secret: Some(SecretRef::parse("op://Agent/kurama-zendesk/client_secret").unwrap()),
        scopes: vec![],
        env_var: "ZENDESK_ACCESS_TOKEN".into(),
        redirect_port: None,
    });
    let plan = planned(
        "zendesk",
        &request(None, &[("subdomain", "example")]),
        &existing(&apis, Some(&held)),
    );
    assert_eq!(plan.auth_action, AuthAction::Reuse);
    assert!(
        plan.setup
            .iter()
            .any(|step| step.ends_with("kurama login --force zendesk")),
        "{:#?}",
        plan.setup
    );
}

/// Another grant, or other explicit endpoints, is another auth contract.
#[test]
fn a_grant_or_endpoint_mismatch_is_refused() {
    let apis = [];
    let refuse = |preset: &str, held: &AuthSource| {
        plan_preset(
            find_preset(preset).unwrap(),
            &PresetRequest::default(),
            &existing(&apis, Some(held)),
            "d",
        )
        .unwrap_err()
        .to_string()
    };
    let device = match google(&[], "id-1") {
        AuthSource::OAuth(mut client) => {
            client.grant_type = GrantType::DeviceCode;
            AuthSource::OAuth(client)
        }
        AuthSource::Token(_) | AuthSource::Secrets(_) => unreachable!(),
    };
    assert!(
        refuse("google-sheets", &device)
            .contains("it uses grant_type = \"device_code\", the preset \"authorization_code\"")
    );
    let github = |token_url: &str, device_auth_url: &str| {
        AuthSource::OAuth(OAuthClientConfig {
            name: "github".into(),
            grant_type: GrantType::DeviceCode,
            endpoints: EndpointSource::Explicit(crate::domain::types::OAuthEndpoints {
                auth_url: None,
                token_url: token_url.into(),
                device_auth_url: Some(device_auth_url.into()),
            }),
            client_id: "Iv1.x".into(),
            client_secret: None,
            scopes: vec!["read:user".into(), "read:org".into()],
            env_var: "GITHUB_TOKEN".into(),
            redirect_port: None,
        })
    };
    const TOKEN: &str = "https://github.com/login/oauth/access_token";
    const DEVICE: &str = "https://github.com/login/device/code";
    let same = planned(
        "github-oauth",
        &PresetRequest::default(),
        &existing(&apis, Some(&github(TOKEN, DEVICE))),
    );
    assert_eq!(same.auth_action, AuthAction::Reuse);
    for held in [
        github("https://ghe.example.com/login/oauth/access_token", DEVICE),
        github(TOKEN, "https://ghe.example.com/login/device/code"),
    ] {
        assert!(
            refuse("github-oauth", &held).contains("its issuer or endpoints are not the preset's")
        );
    }
}

/// The GitHub form asks for a callback URL the device flow never uses, and
/// a Google Web application client needs the redirect kurama derives.
#[test]
fn the_setup_says_what_the_provider_forms_need() {
    let apis = [];
    let github = planned(
        "github-oauth",
        &PresetRequest::default(),
        &existing(&apis, None),
    );
    assert!(
        github.setup[0].contains("any URL will do"),
        "{}",
        github.setup[0]
    );
    for id in ["google-sheets", "google-docs", "google-drive"] {
        let plan = planned(id, &PresetRequest::default(), &existing(&apis, None));
        assert!(
            plan.setup[0].contains(&format!(
                "A Web application client must register exactly {}",
                redirect_uri(8080)
            )),
            "{}",
            plan.setup[0]
        );
    }
}

/// The missing-input hint names where the inputs and the steps are listed.
#[test]
fn the_missing_input_hint_names_both_outputs() {
    let hint = PresetError::MissingInputs {
        id: "x".into(),
        keys: vec![],
    }
    .hint();
    assert!(!hint.contains("above"), "{hint}");
    assert!(hint.contains("kurama preset setup <ID>"), "{hint}");
}

/// Each value a setup command carries from `--set`, `--as` or the file's
/// vault reaches the shell as one word, whatever spaces, quotes or `;` it
/// holds: a pasted step runs nothing it was not meant to.
#[test]
fn setup_commands_shell_quote_dynamic_values() {
    let apis = [];
    let vault = "My 'Vault'";
    let site = "my site;rm x";
    let plan = planned(
        "jira",
        &request(Some("work"), &[("site", site), ("email", "a b@c.d")]),
        &Existing {
            vault: Some(vault),
            ..existing(&apis, None)
        },
    );
    let lines: Vec<&str> = plan.setup.iter().flat_map(|step| step.lines()).collect();
    let words = |start: &str| -> Vec<String> {
        let line = lines
            .iter()
            .find(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no {start} line: {lines:?}"));
        shell_words::split(line).unwrap_or_else(|error| panic!("{line}: {error}"))
    };
    let store = words("op item create");
    assert!(
        store.windows(2).any(|pair| pair == ["--vault", vault]),
        "{store:?}"
    );
    let add = words("kurama preset setup");
    for (flag, value) in [
        ("--as", "work".to_owned()),
        ("--set", format!("site={site}")),
        ("--set", "email=a b@c.d".to_owned()),
        (
            "--set",
            format!("secret=op://{vault}/kurama-jira/credential"),
        ),
    ] {
        assert!(
            add.windows(2).any(|pair| pair == [flag, value.as_str()]),
            "{flag} {value}: {add:?}"
        );
    }
    let call = words("kurama api ");
    assert_eq!(call[2], "work", "{call:?}");
}

/// Only the inputs a URL is built from are the ones a refused section is
/// blamed on: the site of Jira, the subdomain of Zendesk, none of GitHub.
#[test]
fn url_inputs_name_the_inputs_a_url_is_built_from() {
    let apis = [];
    for (id, expected) in [
        ("jira", vec!["site"]),
        ("zendesk", vec!["subdomain"]),
        ("github", vec![]),
    ] {
        let preset = find_preset(id).unwrap();
        let plan = planned(id, &full_request(preset), &existing(&apis, None));
        assert_eq!(plan.url_inputs, expected, "{id}");
    }
}
