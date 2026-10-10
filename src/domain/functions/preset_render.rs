//! A preset expanded against the configuration: the names, whether an existing `[auth.*]` is reused, the TOML to append, the setup steps and the warnings, for `preset setup`.
//!
//! [`plan_preset`] decides everything from the preset, what was given with
//! `--as` / `--auth-as` / `--set` and what the configuration already has,
//! and nothing else: it resolves no secret and reads no file. An existing
//! `[auth.*]` of the auth's name (the preset's, or `--auth-as`) is reused
//! only when its contract is the preset's --
//! kind, grant, issuer or endpoints, token header and format -- and a
//! `client_id` or secret reference given with `--set` is the one it holds;
//! references are compared as written. Scopes it lacks are a warning. A
//! value a setup command carries from `--set`, `--as` or the vault is one
//! shell word, quoted when it needs to be.

use std::collections::BTreeMap;

use super::oauth::redirect_uri;
use super::operation_command::shell_quote;
use crate::domain::types::preset::{
    AuthPresetKind, Preset, PresetEndpoints, PresetPlacement, PresetSpec,
};
use crate::domain::types::{
    AuthSource, DEFAULT_TOKEN_FORMAT, DEFAULT_TOKEN_HEADER, EndpointSource, SecretRef,
    TokenPlacement,
};

/// What `--as`, `--auth-as` and `--set` asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresetRequest {
    /// The `[api.*]` name; the preset's own when `None`.
    pub api_name: Option<String>,
    /// The `[auth.*]` name; the preset's own when `None`.
    pub auth_name: Option<String>,
    /// `--set key=value`, the last one of a key winning.
    pub inputs: BTreeMap<String, String>,
}

/// What the configuration already holds that the plan depends on.
#[derive(Debug, Clone, Copy)]
pub struct Existing<'a> {
    /// Every `[api.*]` name.
    pub apis: &'a [String],
    /// The `[auth.*]` of the auth name the plan uses, when there is one.
    pub auth: Option<&'a AuthSource>,
    /// `[onepassword] vault`, for the suggested `op://` references.
    pub vault: Option<&'a str>,
    /// The file the setup tells to append to.
    pub config_path: &'a str,
}

/// A refusal. `NotFound` is `PRESET_NOT_FOUND`, an auth named like an AWS
/// profile `CONFIG_INVALID` (the fix is `--auth-as` or ~/.aws/config), the
/// rest `ARGUMENT_INVALID`: `--set`, `--as` or `--auth-as` is what changes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresetError {
    #[error("no preset is named {0}")]
    NotFound(String),
    #[error("preset {id} takes no input {key}; it takes {accepted}")]
    UnknownInput {
        id: String,
        key: String,
        accepted: String,
    },
    #[error("preset {id} needs {}; give each with --set <key>=<value>", keys.join(", "))]
    MissingInputs { id: String, keys: Vec<String> },
    /// Secret inputs given as the value itself, by their `--set` key. The
    /// value is never kept.
    #[error("--set {} must be a secret reference, not the value itself; the value is not shown", keys.join(", "))]
    LiteralSecret { keys: Vec<String> },
    #[error("[api.{name}] is already in {path}")]
    ApiTaken { name: String, path: String },
    /// The auth a preset would add is named like an AWS profile.
    #[error("[auth.{0}] has the same name as the AWS profile '{0}' in ~/.aws/config")]
    AuthNamedLikeAwsProfile(String),
    #[error("[auth.{name}] in {path} cannot be reused by preset {id}: {reason}")]
    AuthIncompatible {
        name: String,
        path: String,
        id: String,
        reason: String,
    },
}

impl PresetError {
    pub fn hint(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "run `kurama preset` to list the presets",
            Self::UnknownInput { .. } | Self::MissingInputs { .. } => {
                "`kurama preset --json` lists each preset's inputs and its setup page; the `next` of `kurama preset setup <ID>`'s configure step says how to make the missing ones"
            }
            Self::AuthNamedLikeAwsProfile(_) => {
                "give the auth another name with --auth-as <name>, or rename that AWS profile in ~/.aws/config; the file was not changed"
            }
            Self::LiteralSecret { .. } => {
                "give op://<vault>/<item>/<field>, aws-secrets://<aws-profile>/<secret-id> or aws-ssm://<aws-profile>/<parameter-name>, or leave the secret out when the file's [auth.*] is reused; the file was not changed"
            }
            Self::ApiTaken { .. } => "give the new API another name with --as <name>",
            Self::AuthIncompatible { .. } => {
                "give the auth another name with --auth-as <name>, replace that [auth.*] with `kurama config set --file`, or remove it with `kurama config remove`"
            }
        }
    }
}

/// Whether the preset's `[auth.*]` is written or the file's is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAction {
    Add,
    Reuse,
}

impl AuthAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Reuse => "reuse",
        }
    }
}

/// The preset expanded against the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetPlan {
    pub id: &'static str,
    pub api: String,
    pub auth: String,
    pub auth_action: AuthAction,
    /// Numbered in the text output; a step may span lines.
    pub setup: Vec<String>,
    /// Where the steps after appending the sections start: what is left to
    /// do once `preset setup` saved them.
    pub after_append: usize,
    /// Scopes a reused auth lacks.
    pub warnings: Vec<String>,
    /// The inputs a URL of the sections is built from: the only values the
    /// checks of config.toml can refuse (a secret is a reference already,
    /// the rest are TOML strings).
    pub url_inputs: Vec<String>,
    /// The TOML to append: provenance comment, then the sections. The
    /// inputs it still needs when some were not given.
    pub fragment: Result<String, PresetError>,
}

/// Expand `preset` for `request` against `existing`. `today` is the date the
/// provenance comment records (`2026-09-26`).
pub fn plan_preset(
    preset: &'static Preset,
    request: &PresetRequest,
    existing: &Existing,
    today: &str,
) -> Result<PresetPlan, PresetError> {
    if let Some(key) = request
        .inputs
        .keys()
        .find(|key| preset.input(key).is_none())
    {
        return Err(PresetError::UnknownInput {
            id: preset.id.to_owned(),
            key: key.clone(),
            accepted: input_keys(preset).join(", "),
        });
    }
    let literal: Vec<String> = request
        .inputs
        .iter()
        .filter(|(key, value)| {
            preset
                .input(key)
                .is_some_and(|input| input.secret_field.is_some())
                && !is_reference(value)
        })
        .map(|(key, _)| key.clone())
        .collect();
    if !literal.is_empty() {
        return Err(PresetError::LiteralSecret { keys: literal });
    }
    let api = request
        .api_name
        .clone()
        .unwrap_or_else(|| preset.api.name.to_owned());
    let auth = request.auth_name(preset).to_owned();
    if existing.apis.contains(&api) {
        return Err(PresetError::ApiTaken {
            name: api,
            path: existing.config_path.to_owned(),
        });
    }
    let (auth_action, missing) = match existing.auth {
        None => (AuthAction::Add, Vec::new()),
        Some(source) => {
            let reason = incompatibility(preset, &request.inputs, source);
            if let Some(reason) = reason {
                return Err(PresetError::AuthIncompatible {
                    name: auth,
                    path: existing.config_path.to_owned(),
                    id: preset.id.to_owned(),
                    reason,
                });
            }
            (AuthAction::Reuse, missing_scopes(preset, source))
        }
    };
    let scope_command = (!missing.is_empty()).then(|| scopes_command(&auth, existing, &missing));
    let warnings = scope_warnings(&auth, &missing, scope_command.as_deref());
    let names = Names {
        api: &api,
        renamed: request.api_name.is_some(),
        auth: &auth,
        auth_renamed: request.auth_name.is_some(),
        auth_action,
        missing_scopes: missing,
        scope_command,
    };
    let (setup, after_append) = setup_steps(preset, request, existing, &names);
    Ok(PresetPlan {
        id: preset.id,
        api: api.clone(),
        auth: auth.clone(),
        auth_action,
        setup,
        after_append,
        warnings,
        url_inputs: url_inputs(preset),
        fragment: fragment(preset, &request.inputs, &names, today),
    })
}

impl PresetRequest {
    /// The `[auth.*]` name the plan uses: `--auth-as`, else the preset's.
    pub fn auth_name<'a>(&'a self, preset: &'a Preset) -> &'a str {
        self.auth_name.as_deref().unwrap_or(preset.auth.name)
    }
}

/// Every input key a preset declares, in its order.
pub fn input_keys(preset: &Preset) -> Vec<&'static str> {
    preset.inputs.iter().map(|input| input.key).collect()
}

struct Names<'a> {
    api: &'a str,
    renamed: bool,
    auth: &'a str,
    auth_renamed: bool,
    auth_action: AuthAction,
    /// Scopes a reused auth lacks: a step adds them before the login.
    missing_scopes: Vec<&'static str>,
    /// The `kurama config set` that adds them, when there are any.
    scope_command: Option<String>,
}

/// Why `source` is not the auth `preset` needs, or `None` when it is.
fn incompatibility(
    preset: &Preset,
    inputs: &BTreeMap<String, String>,
    source: &AuthSource,
) -> Option<String> {
    let given = |template: &str| template_value(template, inputs);
    let differs = |key: &str| format!("the {key} given with --set is not the one it holds");
    match (&preset.auth.kind, source) {
        (
            AuthPresetKind::OAuth {
                grant_type,
                endpoints,
                client_id,
                client_secret,
                ..
            },
            AuthSource::OAuth(client),
        ) => {
            if client.grant_type != *grant_type {
                return Some(format!(
                    "it uses grant_type = \"{}\", the preset \"{}\"",
                    client.grant_type.as_str(),
                    grant_type.as_str()
                ));
            }
            if !same_endpoints(endpoints, &client.endpoints, inputs) {
                return Some("its issuer or endpoints are not the preset's".to_owned());
            }
            if given(client_id).is_some_and(|value| value != client.client_id) {
                return Some(differs("client_id"));
            }
            let secret = client_secret.and_then(given);
            if secret.is_some_and(|value| !same_reference(&value, client.client_secret.as_ref())) {
                return Some(differs("client_secret"));
            }
            None
        }
        (AuthPresetKind::Token { token, placement }, AuthSource::Token(source)) => {
            if let Some(reason) = placement_difference(placement, &source.placement, inputs) {
                return Some(reason);
            }
            if given(token).is_some_and(|value| !same_reference(&value, Some(&source.token))) {
                return Some(differs("secret"));
            }
            None
        }
        (kind, source) => Some(format!(
            "it is kind = \"{}\", the preset {}",
            source.kind().as_str(),
            kind.summary()
        )),
    }
}

/// Why `source` puts the credential somewhere else than `preset` would, or
/// `None` when it puts it in the same place. A username the inputs do not
/// fill in is not compared: reusing the auth needs no `--set` for it.
fn placement_difference(
    preset: &PresetPlacement,
    source: &TokenPlacement,
    inputs: &BTreeMap<String, String>,
) -> Option<String> {
    match (preset, source) {
        (
            PresetPlacement::Header { header, format },
            TokenPlacement::Header { name, format: held },
        ) => {
            let (header, format) = (
                header.unwrap_or(DEFAULT_TOKEN_HEADER),
                format.unwrap_or(DEFAULT_TOKEN_FORMAT),
            );
            (name != header || held != format).then(|| {
                format!("it sends the token as {name}: {held}, the preset as {header}: {format}")
            })
        }
        (PresetPlacement::Basic { username }, TokenPlacement::Basic { username: held }) => {
            template_value(username, inputs)
                .is_some_and(|value| value != *held)
                .then(|| "the username given with --set is not the one it holds".to_owned())
        }
        (PresetPlacement::Query { param }, TokenPlacement::Query { param: held })
            if param == held =>
        {
            None
        }
        _ => Some(format!(
            "it sends the token as {}, the preset as {}",
            source.summary(),
            preset.summary()
        )),
    }
}

/// `template` with the given inputs, when every one it names was given.
fn template_value(template: &str, inputs: &BTreeMap<String, String>) -> Option<String> {
    let names = template_variables(template);
    names
        .iter()
        .all(|name| inputs.contains_key(name))
        .then(|| fill(template, |name| inputs.get(name).cloned()))
}

/// `value` is a secret reference (`op://`, `aws-secrets://`, `aws-ssm://`).
fn is_reference(value: &str) -> bool {
    SecretRef::parse(value).is_ok_and(|secret| secret.is_reference())
}

/// The two references are the same as written; nothing is resolved.
fn same_reference(given: &str, held: Option<&SecretRef>) -> bool {
    match (SecretRef::parse(given), held) {
        (Ok(given), Some(held)) => &given == held,
        _ => false,
    }
}

fn same_endpoints(
    preset: &PresetEndpoints,
    held: &EndpointSource,
    inputs: &BTreeMap<String, String>,
) -> bool {
    let value = |template: &str| fill(template, |name| inputs.get(name).cloned());
    match (preset, held) {
        (PresetEndpoints::Issuer(issuer), EndpointSource::Issuer(held)) => {
            value(issuer).trim_end_matches('/') == held.trim_end_matches('/')
        }
        (
            PresetEndpoints::Explicit {
                auth_url,
                token_url,
                device_auth_url,
            },
            EndpointSource::Explicit(held),
        ) => {
            value(token_url) == held.token_url
                && auth_url.map(value) == held.auth_url
                && device_auth_url.map(value) == held.device_auth_url
        }
        _ => false,
    }
}

/// The scopes the preset asks for that a reused OAuth auth lacks; none for
/// a token auth.
fn missing_scopes(preset: &Preset, source: &AuthSource) -> Vec<&'static str> {
    let (AuthPresetKind::OAuth { scopes, .. }, AuthSource::OAuth(client)) =
        (&preset.auth.kind, source)
    else {
        return Vec::new();
    };
    scopes
        .iter()
        .copied()
        .filter(|scope| !client.scopes.iter().any(|held| held == scope))
        .collect()
}

/// The `kurama config set` that gives the reused auth every scope it holds
/// and the ones it lacks: an array is replaced whole.
fn scopes_command(auth: &str, existing: &Existing, missing: &[&str]) -> String {
    let held = match existing.auth {
        Some(AuthSource::OAuth(client)) => client.scopes.clone(),
        _ => Vec::new(),
    };
    let scopes: Vec<String> = held
        .into_iter()
        .chain(missing.iter().map(|scope| (*scope).to_owned()))
        .collect();
    // A JSON array of strings is a TOML array.
    let list = serde_json::to_string(&scopes).expect("strings serialize");
    format!(
        "kurama config set auth.{auth}.scopes {}",
        shell_quote(&list)
    )
}

/// The missing scopes as one warning.
fn scope_warnings(auth: &str, missing: &[&str], command: Option<&str>) -> Vec<String> {
    let Some(command) = command else {
        return Vec::new();
    };
    vec![format!(
        "[auth.{name}] lacks the scope{s} {list}: add {them} with `{command}`, then run `kurama login --force {name}`",
        name = auth,
        s = if missing.len() == 1 { "" } else { "s" },
        list = missing.join(", "),
        them = if missing.len() == 1 { "it" } else { "them" },
    )]
}

/// The numbered steps: creating the credential (unless the auth is reused),
/// the API's own, appending the TOML, logging in and a first call; and where
/// the steps after the append start.
fn setup_steps(
    preset: &Preset,
    request: &PresetRequest,
    existing: &Existing,
    names: &Names,
) -> (Vec<String>, usize) {
    let vault = existing.vault.unwrap_or("<vault>");
    let item = preset.auth.item();
    let redirect = match &preset.auth.kind {
        AuthPresetKind::OAuth { redirect_port, .. } => {
            redirect_port.map(redirect_uri).unwrap_or_default()
        }
        AuthPresetKind::Token { .. } => String::new(),
    };
    let variables = |name: &str| -> Option<String> {
        Some(match name {
            "docs_url" => preset.docs_url.to_owned(),
            "redirect_uri" => redirect.clone(),
            // Only the store command names the vault: a shell word there.
            "vault" => shell_quote(vault),
            "item" => item.clone(),
            _ => return None,
        })
    };
    let mut steps = Vec::new();
    if names.auth_action == AuthAction::Add {
        steps.extend(preset.auth.setup.iter().map(|step| fill(step, variables)));
    }
    steps.extend(preset.setup.iter().map(|step| fill(step, variables)));
    let mut command = format!("kurama preset setup {}", preset.id);
    if names.renamed {
        command.push_str(&format!(" --as {}", shell_quote(names.api)));
    }
    if names.auth_renamed {
        command.push_str(&format!(" --auth-as {}", shell_quote(names.auth)));
    }
    for key in needed_inputs(preset, names.auth_action) {
        let input = preset
            .input(key)
            .expect("a template names a declared input");
        // A secret given as a value is never echoed: the step shows the
        // reference to store it under instead.
        let given = request.inputs.get(key);
        let value = match (given, input.secret_field) {
            (Some(value), _) => value.clone(),
            (None, Some(field)) => format!("op://{vault}/{item}/{field}"),
            (None, None) => format!("<{}>", key.replace('_', "-")),
        };
        command.push_str(&format!(
            " --set {}",
            shell_quote(&format!("{key}={value}"))
        ));
    }
    steps.push(format!(
        "Set it up -- append the sections to {}, check them and try the credential:\n{command}",
        existing.config_path
    ));
    let after_append = steps.len();
    let scopes_short = !names.missing_scopes.is_empty();
    if let Some(command) = &names.scope_command {
        steps.push(format!(
            "Add {} to the scopes of [auth.{}]:\n{command}",
            names.missing_scopes.join(", "),
            names.auth,
        ));
    }
    // A client credentials grant needs no person, so the first call gets its
    // token; only a token stored before the scopes grew has to be replaced.
    let first_login = match preset.auth.kind {
        AuthPresetKind::OAuth { grant_type, .. } => grant_type.needs_human(),
        AuthPresetKind::Token { .. } => false,
    };
    if matches!(preset.auth.kind, AuthPresetKind::OAuth { .. })
        && ((first_login && names.auth_action == AuthAction::Add) || scopes_short)
    {
        let force = if scopes_short { " --force" } else { "" };
        steps.push(format!(
            "Authorize kurama:\nkurama login{force} {}",
            shell_quote(names.auth)
        ));
    }
    steps.push(format!(
        "Call the API:\nkurama api {} {}",
        shell_quote(names.api),
        preset.api.example
    ));
    (steps, after_append)
}

/// The inputs the sections a plan writes name, in declaration order.
fn needed_inputs(preset: &Preset, auth_action: AuthAction) -> Vec<&'static str> {
    let mut named = template_variables(preset.api.base_url);
    if let Some(spec) = preset.api.spec {
        named.extend(template_variables(spec.value()));
    }
    if auth_action == AuthAction::Add {
        named.extend(auth_templates(&preset.auth.kind).flat_map(template_variables));
    }
    input_keys(preset)
        .into_iter()
        .filter(|key| named.iter().any(|name| name == key))
        .collect()
}

/// The inputs the API's URLs are built from, in declaration order. No
/// preset builds an auth URL from an input.
fn url_inputs(preset: &Preset) -> Vec<String> {
    let mut urls = vec![preset.api.base_url];
    urls.extend(preset.api.spec.and_then(PresetSpec::url));
    let named: Vec<String> = urls.into_iter().flat_map(template_variables).collect();
    input_keys(preset)
        .into_iter()
        .filter(|key| named.iter().any(|name| name == key))
        .map(str::to_owned)
        .collect()
}

/// Every templated value of an auth preset.
pub fn auth_templates(kind: &AuthPresetKind) -> impl Iterator<Item = &'static str> {
    let values: Vec<&'static str> = match kind {
        AuthPresetKind::OAuth {
            endpoints,
            client_id,
            client_secret,
            ..
        } => {
            let mut values = vec![*client_id];
            values.extend(*client_secret);
            match endpoints {
                PresetEndpoints::Issuer(issuer) => values.push(*issuer),
                PresetEndpoints::Explicit {
                    auth_url,
                    token_url,
                    device_auth_url,
                } => {
                    values.push(*token_url);
                    values.extend(*auth_url);
                    values.extend(*device_auth_url);
                }
            }
            values
        }
        AuthPresetKind::Token { token, placement } => match placement {
            PresetPlacement::Basic { username } => vec![*token, *username],
            PresetPlacement::Header { .. } | PresetPlacement::Query { .. } => vec![*token],
        },
    };
    values.into_iter()
}

/// The TOML to append, or the inputs it still needs.
fn fragment(
    preset: &Preset,
    inputs: &BTreeMap<String, String>,
    names: &Names,
    today: &str,
) -> Result<String, PresetError> {
    let missing: Vec<String> = needed_inputs(preset, names.auth_action)
        .into_iter()
        .filter(|key| !inputs.contains_key(*key))
        .map(str::to_owned)
        .collect();
    if !missing.is_empty() {
        return Err(PresetError::MissingInputs {
            id: preset.id.to_owned(),
            keys: missing,
        });
    }
    let value = |template: &str| quote(&fill(template, |name| inputs.get(name).cloned()));
    let mut out = format!(
        "# kurama preset: {} (kurama {}, {today})\n# setup: {}\n",
        preset.id,
        env!("CARGO_PKG_VERSION"),
        preset.docs_url
    );
    if names.auth_action == AuthAction::Add {
        let auth = &preset.auth;
        out.push_str(&format!("[auth.{}]\n", names.auth));
        match &auth.kind {
            AuthPresetKind::OAuth {
                grant_type,
                endpoints,
                client_id,
                client_secret,
                scopes,
                redirect_port,
            } => {
                out.push_str("kind = \"oauth\"\n");
                out.push_str(&format!("grant_type = \"{}\"\n", grant_type.as_str()));
                match endpoints {
                    PresetEndpoints::Issuer(issuer) => {
                        out.push_str(&format!("issuer = {}\n", value(issuer)));
                    }
                    PresetEndpoints::Explicit {
                        auth_url,
                        token_url,
                        device_auth_url,
                    } => {
                        if let Some(url) = auth_url {
                            out.push_str(&format!("auth_url = {}\n", value(url)));
                        }
                        out.push_str(&format!("token_url = {}\n", value(token_url)));
                        if let Some(url) = device_auth_url {
                            out.push_str(&format!("device_auth_url = {}\n", value(url)));
                        }
                    }
                }
                out.push_str(&format!("client_id = {}\n", value(client_id)));
                if let Some(secret) = client_secret {
                    out.push_str(&format!("client_secret = {}\n", value(secret)));
                }
                let scopes: Vec<String> = scopes.iter().map(|scope| quote(scope)).collect();
                out.push_str(&format!("scopes = [{}]\n", scopes.join(", ")));
                if let Some(port) = redirect_port {
                    out.push_str(&format!("redirect_port = {port}\n"));
                }
            }
            AuthPresetKind::Token { token, placement } => {
                out.push_str("kind = \"token\"\n");
                out.push_str(&format!("token = {}\n", value(token)));
                match placement {
                    PresetPlacement::Header { header, format } => {
                        if let Some(header) = header {
                            out.push_str(&format!("header = {}\n", quote(header)));
                        }
                        if let Some(format) = format {
                            out.push_str(&format!("format = {}\n", quote(format)));
                        }
                    }
                    PresetPlacement::Basic { username } => {
                        out.push_str(&format!("username = {}\n", value(username)));
                    }
                    PresetPlacement::Query { param } => {
                        out.push_str(&format!("query = {}\n", quote(param)));
                    }
                }
            }
        }
        if let Some(env_var) = auth.env_var {
            out.push_str(&format!("env_var = {}\n", quote(env_var)));
        }
        out.push('\n');
    }
    let api = &preset.api;
    out.push_str(&format!("[api.{}]\n", names.api));
    out.push_str(&format!("description = {}\n", quote(api.description)));
    out.push_str(&format!("base_url = {}\n", value(api.base_url)));
    out.push_str(&format!("auth = {}\n", quote(names.auth)));
    if let Some(spec) = api.spec {
        out.push_str(&format!("{} = {}\n", spec.key(), value(spec.value())));
    }
    if !api.headers.is_empty() {
        let headers: Vec<String> = api
            .headers
            .iter()
            .map(|(name, value)| format!("{} = {}", quote(name), quote(value)))
            .collect();
        out.push_str(&format!("headers = {{ {} }}\n", headers.join(", ")));
    }
    Ok(out)
}

/// The `{name}` variables of a template, in order: a brace, then letters,
/// digits and `_`, then a brace. JSON in an example (`{"query"`) is not one.
pub fn template_variables(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let length = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        if length > 0 && after[length..].starts_with('}') {
            names.push(after[..length].to_owned());
        }
        rest = after;
    }
    names
}

/// `template` with each variable `value` knows replaced; the others stay.
fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = template.to_owned();
    for name in template_variables(template) {
        if let Some(text) = value(&name) {
            out = out.replace(&format!("{{{name}}}"), &text);
        }
    }
    out
}

/// A TOML basic string.
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
#[path = "preset_render_tests.rs"]
mod tests;
