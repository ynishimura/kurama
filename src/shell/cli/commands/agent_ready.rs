//! `kurama agent ready [--json]`: whether each AWS profile, `[auth.*]`, `[api.*]` and `[db.*]` can be used right now, from the reads `status` makes.
//!
//! It reads `~/.aws/config`, `config.toml`, the session cache and the token
//! store, as `status` does, and never calls STS, a token endpoint, an API or
//! 1Password, nor resolves a secret: whether one could be read is judged from
//! its reference, which is never printed.

use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

use super::source::check_name_collisions;
use super::status::{gather_auth_statuses, gather_profile_statuses};
use crate::adapters::config::{Config, db::DbAuth, db::DbConnection};
use crate::adapters::profile::load_profiles;
use crate::adapters::token_store::create_token_store;
use crate::domain::functions::source_readiness::{
    OnePassword, Readiness, SourceReadiness, auth_readiness, aws_readiness, dependent_readiness,
    named, secret_condition,
};
use crate::shell::cli::client::{json_line, tab_separated};

/// The environment variable the 1Password CLI reads a service account from.
const SERVICE_ACCOUNT_VAR: &str = "OP_SERVICE_ACCOUNT_TOKEN";

#[derive(Serialize)]
struct ReadyOutput {
    schema_version: u8,
    kind: &'static str,
    /// Every source is `ready`.
    ready: bool,
    sources: Vec<SourceReadiness>,
}

pub async fn run(json: bool, config: &Config) -> Result<()> {
    let mut profiles = load_profiles().await?;
    check_name_collisions(&profiles, config)?;
    profiles.sort_by(|a, b| a.name().cmp(b.name()));
    let sources = config.auth_sources()?;
    let apis = config.api_profiles()?;
    let now = Utc::now();
    let one_password = OnePassword {
        mfa_enabled: config.onepassword.enabled,
        headless: std::env::var(SERVICE_ACCOUNT_VAR).is_ok_and(|v| !v.is_empty())
            || config.onepassword.service_account_keychain.is_some(),
    };

    let mut rows: Vec<SourceReadiness> = gather_profile_statuses(&profiles, config, now)
        .await
        .iter()
        .map(|status| aws_readiness(status, one_password, now))
        .collect();
    let aws_rows = rows.clone();
    let aws = |name: &str| named(&aws_rows, "aws", name);
    let statuses = gather_auth_statuses(&sources, create_token_store().as_ref(), now).await;
    for source in &sources {
        let status = &statuses[source.name()];
        rows.push(auth_readiness(source, status, one_password, &aws, now));
    }
    let known = rows.clone();
    for api in &apis {
        let needs = [
            api.auth.as_deref().map(|name| named(&known, "auth", name)),
            api.aws_profile.as_deref().map(aws),
        ];
        rows.push(dependent_readiness(
            &api.name,
            "api",
            needs.into_iter().flatten().collect(),
            "the API is called without a credential",
        ));
    }
    for (name, section) in &config.db {
        let needs = match section.connection() {
            Ok(DbConnection::Server(server)) => {
                let mut needs = vec![secret_condition(&server.username, one_password, &aws)];
                needs.push(match &server.auth {
                    DbAuth::Password(password) => secret_condition(password, one_password, &aws),
                    DbAuth::Iam(iam) => aws(&iam.aws_profile),
                });
                needs.extend(server.tunnel.iter().map(|tunnel| aws(&tunnel.aws_profile)));
                needs
            }
            _ => vec![],
        };
        rows.push(dependent_readiness(
            name,
            "db",
            needs,
            "a local file, opened without a credential",
        ));
    }

    let output = ReadyOutput {
        schema_version: 1,
        kind: "agent_ready",
        ready: rows.iter().all(|row| row.state == Readiness::Ready),
        sources: rows,
    };
    if json {
        println!("{}", json_line(&output));
    } else {
        print!("{}", render_table(&output.sources));
    }
    Ok(())
}

fn render_table(rows: &[SourceReadiness]) -> String {
    let cells: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            vec![
                row.kind.into(),
                row.name.clone().into(),
                serde_json::to_value(row.state).expect("a state serializes"),
                row.reason.clone().into(),
                row.next_actions.join("; ").into(),
            ]
        })
        .collect();
    tab_separated(
        ["kind", "name", "state", "reason", "next_actions"].into_iter(),
        &cells,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ready_table_names_the_state_and_what_to_run() {
        let row = SourceReadiness {
            name: "gh".into(),
            kind: "auth",
            state: Readiness::NeedsHuman,
            reason: "no token is stored".into(),
            expires_at: None,
            next_actions: vec!["kurama login gh".into()],
        };
        assert_eq!(
            render_table(&[row]),
            "kind\tname\tstate\treason\tnext_actions\nauth\tgh\tneeds_human\tno token is stored\tkurama login gh\n"
        );
    }
}
