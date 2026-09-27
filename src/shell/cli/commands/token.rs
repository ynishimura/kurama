//! `kurama token <PROFILE> [--json] [--fingerprint]`: the credential of an
//! `[auth.*]` source on stdout -- an OAuth access token, refreshed or newly
//! granted when needed, or the value a `kind = "token"` source's reference
//! names -- or, with `--fingerprint`, only its SHA-256.
//! The only command besides `env --json` that prints a secret on stdout.

use anyhow::Result;

use super::source::{CredentialSource, ensure_source_credential, resolve_source};
use crate::adapters::config::Config;
use crate::domain::functions::export::{
    generate_fingerprint_json, generate_token_json, token_fingerprint,
};
use crate::shell::api_runtime::ApiRuntimeOptions;
use crate::shell::cli::executor::CliExecutorError;

pub async fn handle_token_command(
    name: &str,
    json: bool,
    fingerprint: bool,
    verbose: bool,
    config: Config,
) -> Result<()> {
    let source = match resolve_source(&config, name).await? {
        CredentialSource::Auth(source) => source,
        CredentialSource::Aws(_) => {
            return Err(CliExecutorError::KindUnsupported {
                name: name.to_string(),
                kind: "aws",
                verb: "token",
            }
            .into());
        }
    };
    let options = ApiRuntimeOptions {
        report_secret_reads: verbose,
        ..ApiRuntimeOptions::default()
    };
    let credential = ensure_source_credential(config, &source, options).await?;
    match (fingerprint, json) {
        (true, true) => println!("{}", generate_fingerprint_json(&credential)),
        (true, false) => println!("{}", token_fingerprint(&credential)),
        (false, true) => println!("{}", generate_token_json(&credential)),
        (false, false) => println!("{}", credential.value()),
    }
    Ok(())
}
