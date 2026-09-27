//! Whether this run is an agent's (`KURAMA_AGENT`), and the `[agent]` refusal of a call made without `--confirm`.
//!
//! A person at a terminal is never held to the policy: only a process whose
//! environment sets `KURAMA_AGENT` to anything but empty or `0` is. The
//! decision is made before any credential is read, so a refused call reaches
//! no STS, no 1Password and no API.

use crate::adapters::config::Config;
use crate::domain::functions::agent_policy::{AgentPolicy, describe_refusal};
use crate::domain::functions::audit::path_of;
use crate::domain::types::http::HttpRequest;

/// The variable that marks a run as an agent's.
pub const AGENT_ENV: &str = "KURAMA_AGENT";

/// Whether this process runs for an agent.
pub fn is_agent_run() -> bool {
    std::env::var(AGENT_ENV).is_ok_and(|value| !value.is_empty() && value != "0")
}

/// A call the `[agent]` policy does not let an agent make on its own.
#[derive(Debug, thiserror::Error)]
#[error("the [agent] policy refuses {0}")]
pub struct AgentPolicyDenied(pub String);

/// The rules this run's requests to the `[api.<api>]` are held to; `None`
/// for a person, or when `--confirm` says a person agreed.
pub fn api_policy(
    config: &Config,
    api: &str,
    agent_run: bool,
    confirmed: bool,
) -> Option<AgentPolicy> {
    (agent_run && !confirmed).then(|| {
        config
            .agent
            .policy_for(config.api.get(api).and_then(|api| api.agent.as_ref()))
    })
}

/// Whether `request` may be sent under `policy`; the path is the URL's,
/// without its query.
pub fn check_api_request(
    policy: Option<&AgentPolicy>,
    request: &HttpRequest,
) -> Result<(), AgentPolicyDenied> {
    let Some(policy) = policy else {
        return Ok(());
    };
    let path = path_of(&request.url);
    policy
        .check(&request.method, &path)
        .map_err(|refusal| AgentPolicyDenied(describe_refusal(&refusal, &request.method, &path)))
}

/// Whether `db --commit` may run: an agent's run needs `--confirm`, whatever
/// `allow_write` says.
pub fn check_db_commit(agent_run: bool, confirmed: bool) -> Result<(), AgentPolicyDenied> {
    if agent_run && !confirmed {
        return Err(AgentPolicyDenied(
            "db --commit: an agent's change is rolled back unless a person confirmed it".into(),
        ));
    }
    Ok(())
}

/// Whether an agent's `exec` on an AWS profile attaches ReadOnlyAccess.
pub fn exec_is_read_only(config: &Config, agent_run: bool, confirmed: bool) -> bool {
    agent_run && !confirmed && config.agent.exec_readonly
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::utils::test_env;

    const CONFIG: &str = "[api.pets]\nbase_url = \"https://pets.example/v1\"\n\n\
                          [api.open]\nbase_url = \"https://open.example\"\n\n\
                          [api.open.agent]\nallow_methods = [\"GET\", \"POST\"]\n";

    fn post(url: &str) -> HttpRequest {
        HttpRequest::new("POST", url)
    }

    fn check(
        config: &Config,
        api: &str,
        request: &HttpRequest,
        agent_run: bool,
        confirmed: bool,
    ) -> Result<(), AgentPolicyDenied> {
        check_api_request(
            api_policy(config, api, agent_run, confirmed).as_ref(),
            request,
        )
    }

    #[test]
    fn a_person_or_a_confirmed_call_is_never_refused() {
        let config = Config::parse(CONFIG).unwrap();
        let request = post("https://pets.example/v1/pets");
        assert!(check(&config, "pets", &request, false, false).is_ok());
        assert!(check(&config, "pets", &request, true, true).is_ok());
        let error = check(&config, "pets", &request, true, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "the [agent] policy refuses POST /v1/pets: POST is not in allow_methods"
        );
    }

    #[test]
    fn the_api_section_widens_the_methods_and_the_query_is_not_the_path() {
        let config = Config::parse(CONFIG).unwrap();
        assert!(
            check(
                &config,
                "open",
                &post("https://open.example/x"),
                true,
                false
            )
            .is_ok()
        );
        let config = Config::parse(&format!(
            "{CONFIG}\n[agent]\ndeny_paths = [\"/v1/admin*\"]\n"
        ))
        .unwrap();
        let get = HttpRequest::new("GET", "https://pets.example/v1/pets?q=/v1/admin");
        assert!(check(&config, "pets", &get, true, false).is_ok());
        let admin = HttpRequest::new("GET", "https://pets.example/v1/admin/users");
        assert!(check(&config, "pets", &admin, true, false).is_err());
    }

    #[test]
    fn db_commit_and_exec_follow_the_agent_run_and_the_confirmation() {
        assert!(check_db_commit(false, false).is_ok());
        assert!(check_db_commit(true, true).is_ok());
        assert!(check_db_commit(true, false).is_err());
        let config = Config::parse("").unwrap();
        assert!(exec_is_read_only(&config, true, false));
        assert!(!exec_is_read_only(&config, false, false));
        assert!(!exec_is_read_only(&config, true, true));
        let config = Config::parse("[agent]\nexec_readonly = false\n").unwrap();
        assert!(!exec_is_read_only(&config, true, false));
    }

    #[test]
    #[serial_test::serial]
    fn an_empty_or_zero_variable_is_not_an_agent_run() {
        let previous = std::env::var_os(AGENT_ENV);
        for (value, expected) in [
            (None, false),
            (Some(""), false),
            (Some("0"), false),
            (Some("1"), true),
        ] {
            test_env::set_or_remove(AGENT_ENV, value);
            assert_eq!(is_agent_run(), expected, "{value:?}");
        }
        test_env::set_or_remove(AGENT_ENV, previous.as_ref());
    }
}
