//! The checks between config.toml and `~/.aws/config` that `Config::parse` cannot make: an `[auth.*]` name that is also an AWS profile, and an `aws_profile` that names none.
//!
//! `kurama config check` reports them and the config writer refuses a
//! candidate that has them, so both read them from here.

use std::path::PathBuf;

use anyhow::Result;

use super::Config;
use crate::adapters::error::CoreError;
use crate::adapters::profile::load_profiles;
use crate::adapters::profile::loader::AwsConfigLoader;
use crate::domain::Profile;

/// What `~/.aws/config` says about the names a configuration uses.
#[derive(Debug)]
pub struct AwsCheck {
    /// `[auth.*]` sections named like an AWS profile, as `auth.<name>`.
    pub collisions: Vec<(String, CoreError)>,
    /// `aws_profile` references that name no profile, with their section;
    /// `None` when the AWS config does not exist and they were not checked.
    pub unknown: Option<Vec<(String, String)>>,
    /// The AWS config file read (`AWS_CONFIG_FILE`, else `~/.aws/config`).
    pub aws_config: PathBuf,
}

/// Read the AWS profiles and check `config` against them. Without an AWS
/// config the one profile is `default`, which the collisions are checked
/// against, and the references are left unchecked rather than called wrong.
pub async fn check_aws(config: &Config) -> Result<AwsCheck> {
    let profiles = load_profiles().await?;
    let (aws_path, _) = AwsConfigLoader::new()?.config_source();
    let collisions = name_collisions(&profiles, config)
        .into_iter()
        .map(|(name, error)| (format!("auth.{name}"), error))
        .collect();
    let unknown = aws_path.exists().then(|| {
        unknown_aws_profiles(&profiles, config)
            .into_iter()
            .map(|(section, profile)| (section, profile.to_owned()))
            .collect()
    });
    Ok(AwsCheck {
        collisions,
        unknown,
        aws_config: aws_path,
    })
}

/// Every `[auth.*]` name that is also an AWS profile name, with the error it is.
pub fn name_collisions(profiles: &[Profile], config: &Config) -> Vec<(String, CoreError)> {
    config
        .auth
        .keys()
        .filter(|name| profiles.iter().any(|profile| profile.name() == *name))
        .map(|name| {
            (
                name.clone(),
                CoreError::config(format!(
                    "[auth.{name}] has the same name as the AWS profile '{name}' in ~/.aws/config; rename one of them"
                )),
            )
        })
        .collect()
}

/// Every `aws_profile` the configuration names, with its section.
pub fn aws_references(config: &Config) -> Vec<(String, &str)> {
    let mut references = Vec::new();
    for (name, api) in &config.api {
        if let Some(profile) = &api.aws_profile {
            references.push((format!("api.{name}"), profile.as_str()));
        }
    }
    for (name, workspace) in &config.data {
        if let Some(profile) = &workspace.aws_profile {
            references.push((format!("data.{name}"), profile.as_str()));
        }
    }
    for (name, connection) in &config.s3 {
        references.push((format!("s3.{name}"), connection.aws_profile.as_str()));
    }
    for (name, section) in &config.db {
        for profile in [
            section.iam.as_ref().map(|iam| iam.aws_profile.as_str()),
            section
                .tunnel
                .as_ref()
                .map(|tunnel| tunnel.aws_profile.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            references.push((format!("db.{name}"), profile));
        }
    }
    references
}

/// The `aws_profile` references that name no profile in `profiles`. Only
/// meaningful when the AWS config exists: without it the one profile is
/// `default`, and every other name is unverified rather than wrong.
fn unknown_aws_profiles<'c>(profiles: &[Profile], config: &'c Config) -> Vec<(String, &'c str)> {
    aws_references(config)
        .into_iter()
        .filter(|(_, profile)| !profiles.iter().any(|known| known.name() == *profile))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[auth.github]\nkind = \"token\"\ntoken = \"op://Agent/gh/credential\"\n\n\
         [api.signed]\nbase_url = \"https://x.execute-api.ap-northeast-1.amazonaws.com\"\naws_profile = \"dev\"\n\n\
         [s3.assets]\naws_profile = \"gone\"\n";

    #[test]
    fn an_auth_named_like_an_aws_profile_collides() {
        let config = Config::parse(CONFIG).unwrap();
        let collisions = name_collisions(&[Profile::new("dev"), Profile::new("github")], &config);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].0, "github");
        assert!(collisions[0].1.to_string().contains("[auth.github]"));
        assert!(name_collisions(&[Profile::new("dev")], &config).is_empty());
    }

    #[test]
    fn only_the_references_to_a_missing_profile_are_unknown() {
        let config = Config::parse(CONFIG).unwrap();
        assert_eq!(
            unknown_aws_profiles(&[Profile::new("dev")], &config),
            [("s3.assets".to_owned(), "gone")]
        );
    }
}
