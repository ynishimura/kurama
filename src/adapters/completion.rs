//! Synchronous, silent completion inputs: configuration, AWS profiles and offline API descriptions.

use std::fs;
use std::sync::{Mutex, OnceLock};

use crate::adapters::config::Config;
use crate::adapters::config::saved::Saved;
use crate::adapters::openapi::{parse_spec, read_offline};
use crate::adapters::profile::{loader::AwsConfigLoader, parser::parse_aws_config};
use crate::domain::functions::completion_candidates::{self, Candidate, ProfileScope};
use crate::domain::types::api_spec::ApiSpec;

struct SpecMemo {
    name: String,
    bytes: Vec<u8>,
    spec: ApiSpec,
}

static SPEC_MEMO: OnceLock<Mutex<Option<SpecMemo>>> = OnceLock::new();

fn read_config() -> Option<Config> {
    let (path, named) = Config::config_source().ok()?;
    match fs::read_to_string(path) {
        Ok(content) => Config::parse(&content).ok(),
        Err(error) if !named && error.kind() == std::io::ErrorKind::NotFound => {
            Some(Config::default())
        }
        Err(_) => None,
    }
}

pub fn profiles(scope: ProfileScope) -> Vec<Candidate> {
    let Some(config) = read_config() else {
        return Vec::new();
    };
    let aws = AwsConfigLoader::new()
        .ok()
        .and_then(|loader| fs::read_to_string(loader.resolve_config_path()).ok())
        .map(|content| parse_aws_config(&content).into_values().collect::<Vec<_>>())
        .unwrap_or_default();
    let auth = config.auth_sources().unwrap_or_default();
    completion_candidates::profile_candidates(&aws, &auth, scope)
}

/// The configured databases, from configuration alone: completion never opens
/// a file, connects or authenticates.
pub fn databases() -> Vec<Candidate> {
    let Some(config) = read_config() else {
        return Vec::new();
    };
    completion_candidates::named_candidates(config.db.iter().filter_map(|(name, section)| {
        let connection = section.connection().ok()?;
        Some((
            name.clone(),
            format!("{} {}", connection.engine().as_str(), connection.database()),
        ))
    }))
}

/// The configured `[s3.*]` connections, each with where it starts.
pub fn s3_connections() -> Vec<Candidate> {
    let Some(config) = read_config() else {
        return Vec::new();
    };
    completion_candidates::named_candidates(config.s3.iter().map(|(name, connection)| {
        let start = match &connection.bucket {
            Some(bucket) => format!(
                "s3://{bucket}/{}",
                connection.prefix.as_deref().unwrap_or_default()
            ),
            None => connection.aws_profile.clone(),
        };
        (name.clone(), start)
    }))
}

/// config.toml parsed for its syntax only: `config show`, `list`, `set`,
/// `unset` and `remove` work on a file that does not load, and so does their
/// completion.
fn saved_config() -> Option<Saved> {
    let (path, _) = Config::config_source().ok()?;
    let content = fs::read_to_string(path).ok()?;
    Saved::parse(&content).ok()
}

/// The sections config.toml holds.
pub fn config_sections() -> Vec<Candidate> {
    let Some(saved) = saved_config() else {
        return Vec::new();
    };
    completion_candidates::named_candidates(
        saved.units().into_iter().map(|unit| (unit, String::new())),
    )
}

/// Every key config.toml holds, as a dotted path from the top
/// (`aws.session_cache.duration`, `api.github.base_url`).
pub fn config_keys() -> Vec<Candidate> {
    let Some(saved) = saved_config() else {
        return Vec::new();
    };
    completion_candidates::named_candidates(saved.units().into_iter().flat_map(|unit| {
        saved
            .keys(&unit)
            .into_iter()
            .map(move |key| (format!("{unit}.{key}"), String::new()))
            .collect::<Vec<_>>()
    }))
}

pub fn apis() -> Vec<Candidate> {
    let Some(config) = read_config() else {
        return Vec::new();
    };
    completion_candidates::named_candidates(
        config
            .api_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|api| (api.name, api.description.unwrap_or(api.base_url))),
    )
}

/// Completion never refreshes a URL: an old cached description is still useful offline.
pub fn read_api_spec(name: &str) -> Option<ApiSpec> {
    let api = read_config()?.api_profile(name).ok()??;
    let spec_source = api.spec?;
    let bytes = read_offline(&spec_source)?;
    let memo = SPEC_MEMO.get_or_init(|| Mutex::new(None));
    if let Some(spec) = memo
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|cached| cached.name == name && cached.bytes == bytes)
        .map(|cached| cached.spec.clone())
    {
        return Some(spec);
    }
    let spec = parse_spec(&spec_source.format, &bytes).ok()?;
    *memo.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SpecMemo {
        name: name.to_string(),
        bytes,
        spec: spec.clone(),
    });
    Some(spec)
}
