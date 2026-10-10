use super::*;
use crate::adapters::config::AuthToml;
use crate::domain::types::SecretRef;
use crate::ports::aws_credentials::MockAwsProfileCredentials;
use crate::ports::http::MockHttpClient;
use crate::ports::secret::MockSecretResolver;
use crate::ports::token_store::MockTokenStore;
use std::sync::atomic::{AtomicUsize, Ordering};

fn config_with_client_credentials() -> Config {
    let mut config = Config::default();
    config.auth.insert(
        "svc".into(),
        toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"client_credentials\"\ntoken_url = \"https://as.example.com/token\"\nclient_id = \"id\"\n",
        )
        .unwrap(),
    );
    config.api.insert(
        "svc".into(),
        toml::from_str("base_url = \"https://api.example.com\"\n").unwrap(),
    );
    config
}

fn config_with_authorization_code() -> Config {
    let mut config = Config::default();
    config.auth.insert(
        "gh".into(),
        toml::from_str::<AuthToml>(
            "kind = \"oauth\"\ngrant_type = \"authorization_code\"\nauth_url = \"https://as.example.com/auth\"\ntoken_url = \"https://as.example.com/token\"\nclient_id = \"id\"\n",
        )
        .unwrap(),
    );
    config.api.insert(
        "gh".into(),
        toml::from_str("base_url = \"https://api.example.com\"\n").unwrap(),
    );
    config
}

fn api(config: &Config) -> ApiProfile {
    config.api_profile("svc").cloned().unwrap()
}

fn token_response(access_token: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: vec![],
        body: format!(r#"{{"access_token":"{access_token}","expires_in":3600}}"#).into_bytes(),
    }
}

#[tokio::test]
async fn a_stored_token_signs_the_request_without_any_token_request() {
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(1)
        .withf(|request| {
            request.url == "https://api.example.com/items"
                && request.header("authorization") == Some("Bearer stored")
        })
        .returning(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: b"[]".to_vec(),
            })
        });
    let mut store = MockTokenStore::new();
    store
        .expect_load()
        .times(1)
        .returning(|_| Ok(Some(OAuthToken::bearer("stored"))));
    let config = config_with_client_credentials();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        store,
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &api(&config),
            HttpRequest::new("GET", "https://api.example.com/items"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
}

#[tokio::test]
async fn a_401_replaces_the_token_once_and_retries_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut http = MockHttpClient::new();
    let counter = Arc::clone(&calls);
    http.expect_send().returning(move |request| {
        let call = counter.fetch_add(1, Ordering::SeqCst);
        if request.url.ends_with("/token") {
            return Ok(token_response(&format!("fresh-{call}")));
        }
        Ok(HttpResponse {
            status: if request.header("authorization") == Some("Bearer stale") {
                401
            } else {
                200
            },
            headers: vec![],
            body: request
                .header("authorization")
                .unwrap_or_default()
                .as_bytes()
                .to_vec(),
        })
    });
    let mut store = MockTokenStore::new();
    store
        .expect_load()
        .returning(|_| Ok(Some(OAuthToken::bearer("stale"))));
    store.expect_store().times(1).returning(|_, _| Ok(()));
    let config = config_with_client_credentials();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        store,
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &api(&config),
            HttpRequest::new("GET", "https://api.example.com/items"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(String::from_utf8_lossy(&response.body).starts_with("Bearer fresh-"));
    // stale request, token request, retried request
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

/// A configuration whose `[auth.issued]` reads its credential from a
/// reference and sends it in `xi-api-key`, with `[api.issued]` using it.
fn config_with_issued_credential() -> Config {
    let mut config = Config::default();
    config.auth.insert(
        "issued".into(),
        toml::from_str::<AuthToml>(
            "kind = \"token\"\ntoken = \"op://Agent/Example/credential\"\nheader = \"xi-api-key\"\nformat = \"{token}\"\n",
        )
        .unwrap(),
    );
    config.api.insert(
        "issued".into(),
        toml::from_str("base_url = \"https://api.example.com\"\n").unwrap(),
    );
    config
}

/// The credential goes in the header the source names, and it comes from the
/// reference: the resolver answers what it was asked for, so a constant
/// could not pass this.
#[tokio::test]
async fn an_issued_credential_is_sent_in_the_header_the_source_names() {
    let mut http = MockHttpClient::new();
    http.expect_send().times(1).returning(|request| {
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: request.header("xi-api-key").unwrap_or_default().into(),
        })
    });
    let mut secrets = MockSecretResolver::new();
    secrets
        .expect_resolve()
        .times(1)
        .returning(|reference| match reference {
            SecretRef::OnePassword(text) => Ok(format!("value-of-{text}").into()),
            other => panic!("unexpected reference {other:?}"),
        });
    let config = config_with_issued_credential();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        // A source of this kind never touches the token store.
        MockTokenStore::new(),
        secrets,
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &config.api_profile("issued").cloned().unwrap(),
            HttpRequest::new("GET", "https://api.example.com/v1/voices"),
        )
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&response.body),
        "value-of-op://Agent/Example/credential"
    );
}

/// Nothing was cached and nothing can be refreshed, so a retry would read
/// the same value and send the same request: the 401 is the answer.
#[tokio::test]
async fn an_issued_credential_is_not_sent_again_after_a_401() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let mut http = MockHttpClient::new();
    http.expect_send().returning(move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: b"no".to_vec(),
        })
    });
    let mut secrets = MockSecretResolver::new();
    secrets
        .expect_resolve()
        .times(1)
        .returning(|_| Ok("issued".into()));
    let config = config_with_issued_credential();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        MockTokenStore::new(),
        secrets,
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &config.api_profile("issued").cloned().unwrap(),
            HttpRequest::new("GET", "https://api.example.com/v1/voices"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 401);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_401_reuses_the_token_another_process_stored_meanwhile() {
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(2)
        .withf(|request| request.url == "https://api.example.com/items")
        .returning(|request| {
            Ok(HttpResponse {
                status: if request.header("authorization") == Some("Bearer stale") {
                    401
                } else {
                    200
                },
                headers: vec![],
                body: request
                    .header("authorization")
                    .unwrap_or_default()
                    .as_bytes()
                    .to_vec(),
            })
        });
    let loads = Arc::new(AtomicUsize::new(0));
    let mut store = MockTokenStore::new();
    let counter = Arc::clone(&loads);
    store.expect_load().returning(move |_| {
        // The first load sees the stale token; another process stored
        // a fresh one by the time the 401 is handled.
        let access_token = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            "stale"
        } else {
            "fresh"
        };
        Ok(Some(OAuthToken::bearer(access_token)))
    });
    let config = config_with_client_credentials();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        store,
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &api(&config),
            HttpRequest::new("GET", "https://api.example.com/items"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"Bearer fresh");
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_401_without_a_terminal_removes_the_dead_token_and_asks_for_a_login() {
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(1)
        .withf(|request| request.url == "https://api.example.com/items")
        .returning(|_| {
            Ok(HttpResponse {
                status: 401,
                headers: vec![],
                body: vec![],
            })
        });
    let mut store = MockTokenStore::new();
    store
        .expect_load()
        .returning(|_| Ok(Some(OAuthToken::bearer("dead"))));
    store
        .expect_remove()
        .times(1)
        .withf(|key| key == "gh")
        .returning(|_| Ok(()));
    let config = config_with_authorization_code();
    let api = config.api_profile("gh").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        http,
        store,
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    let error = runtime
        .call(
            &api,
            HttpRequest::new("GET", "https://api.example.com/items"),
        )
        .await
        .unwrap_err();
    assert!(is_login_required(&error), "{error:#}");
}

#[tokio::test]
async fn a_second_401_is_returned_as_is() {
    let mut http = MockHttpClient::new();
    http.expect_send().returning(|request| {
        if request.url.ends_with("/token") {
            return Ok(token_response("fresh"));
        }
        Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: b"{\"message\":\"Bad credentials\"}".to_vec(),
        })
    });
    let mut store = MockTokenStore::new();
    store
        .expect_load()
        .returning(|_| Ok(Some(OAuthToken::bearer("stale"))));
    store.expect_store().returning(|_, _| Ok(()));
    let config = config_with_client_credentials();
    let runtime = ApiRuntime::test(
        config.clone(),
        http,
        store,
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    let response = runtime
        .call(
            &api(&config),
            HttpRequest::new("GET", "https://api.example.com/items"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 401);
}

#[tokio::test]
async fn an_api_without_auth_sends_no_authorization_header() {
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(1)
        .withf(|request| request.header("authorization").is_none())
        .returning(|_| {
            Ok(HttpResponse {
                status: 204,
                headers: vec![],
                body: vec![],
            })
        });
    let mut config = Config::default();
    config.api.insert(
        "public".into(),
        toml::from_str("base_url = \"https://example.com\"\n").unwrap(),
    );
    let api = config.api_profile("public").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        http,
        MockTokenStore::new(),
        MockSecretResolver::new(),
        MockAwsProfileCredentials::new(),
        false,
    );
    assert_eq!(
        runtime
            .call(&api, HttpRequest::new("GET", "https://example.com/"))
            .await
            .unwrap()
            .status,
        204
    );
}

fn config_with_aws_profile(api: &str) -> Config {
    let mut config = Config::default();
    config
        .api
        .insert("apigw".into(), toml::from_str(api).unwrap());
    config
}

fn dev_profile() -> Profile {
    Profile::new("dev")
        .with_role_arn_raw("arn:aws:iam::123456789012:role/Dev")
        .with_region_raw("us-east-1")
}

fn role_credentials() -> Credentials {
    Credentials::new(
        "ASIAROLEKEY".into(),
        "role-secret".into(),
        Some("role-session-token".into()),
        None,
    )
}

/// `Credential=<key>/<date>/<region>/<service>/aws4_request` of a signed request.
fn credential_scope(request: &HttpRequest) -> String {
    request
        .header("authorization")
        .and_then(|value| value.split_once("Credential="))
        .and_then(|(_, rest)| rest.split_once(','))
        .map(|(scope, _)| scope.to_string())
        .unwrap_or_default()
}

#[tokio::test]
async fn an_aws_profile_api_is_signed_with_the_role_credentials_and_sent_once() {
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(1)
        .withf(|request| {
            request.url == "https://abc.execute-api.ap-northeast-1.amazonaws.com/prod/items"
                && credential_scope(request).starts_with("ASIAROLEKEY/")
                && credential_scope(request).ends_with("/ap-northeast-1/execute-api/aws4_request")
                && request.header("x-amz-security-token") == Some("role-session-token")
                && request.header("x-amz-date").is_some()
                && request.header("accept") == Some("application/json")
        })
        .returning(|_| {
            Ok(HttpResponse {
                status: 403,
                headers: vec![],
                body: b"{\"message\":\"Forbidden\"}".to_vec(),
            })
        });
    let mut aws = MockAwsProfileCredentials::new();
    aws.expect_load_profile()
        .times(1)
        .withf(|name| name == "dev")
        .returning(|_| Ok(dev_profile()));
    aws.expect_assume_role()
        .times(1)
        .withf(|profile| profile.name() == "dev")
        .returning(|_| Ok(role_credentials()));
    let config = config_with_aws_profile(
        "base_url = \"https://abc.execute-api.ap-northeast-1.amazonaws.com/prod\"\naws_profile = \"dev\"\n",
    );
    let api = config.api_profile("apigw").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        http,
        MockTokenStore::new(),
        MockSecretResolver::new(),
        aws,
        false,
    );
    let request = HttpRequest::new(
        "GET",
        "https://abc.execute-api.ap-northeast-1.amazonaws.com/prod/items",
    )
    .with_header("Accept", "application/json");
    // The 403 is returned as is: no retry, no second AssumeRole.
    assert_eq!(runtime.call(&api, request).await.unwrap().status, 403);
}

#[tokio::test]
async fn the_profile_service_and_the_aws_profile_region_fill_in_for_a_custom_domain() {
    // No API region: it must come from the AWS profile.
    let mut http = MockHttpClient::new();
    http.expect_send()
        .times(1)
        .withf(|request| credential_scope(request).ends_with("/us-east-1/execute-api/aws4_request"))
        .returning(|_| {
            Ok(HttpResponse {
                status: 200,
                headers: vec![],
                body: vec![],
            })
        });
    let mut aws = MockAwsProfileCredentials::new();
    aws.expect_load_profile().returning(|_| Ok(dev_profile()));
    aws.expect_assume_role()
        .returning(|_| Ok(role_credentials()));
    let config = config_with_aws_profile(
        "base_url = \"https://api.example.com\"\naws_profile = \"dev\"\nservice = \"execute-api\"\n",
    );
    let api = config.api_profile("apigw").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        http,
        MockTokenStore::new(),
        MockSecretResolver::new(),
        aws,
        false,
    );
    let request = HttpRequest::new("GET", "https://api.example.com/items");
    assert_eq!(runtime.call(&api, request).await.unwrap().status, 200);
}

#[tokio::test]
async fn an_open_signing_target_stops_before_any_credential_is_requested() {
    let mut aws = MockAwsProfileCredentials::new();
    aws.expect_load_profile()
        .returning(|_| Ok(Profile::new("dev")));
    aws.expect_assume_role().times(0);
    let config =
        config_with_aws_profile("base_url = \"https://api.example.com\"\naws_profile = \"dev\"\n");
    let api = config.api_profile("apigw").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        MockHttpClient::new(),
        MockTokenStore::new(),
        MockSecretResolver::new(),
        aws,
        false,
    );
    let request = HttpRequest::new("GET", "https://api.example.com/items");
    for result in [
        runtime.call(&api, request.clone()).await.map(|_| ()),
        runtime.credential(&api, &request).await.map(|_| ()),
    ] {
        let error = result.unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<ApiError>(),
                Some(ApiError::SigningTargetRequired { api, message })
                    if api == "apigw" && message.contains("service and region")
            ),
            "{error:#}"
        );
    }
}

/// The request as `--dry-run` shows it: the credential decided, then the
/// placeholders put in.
async fn preview(runtime: &ApiRuntime, api: &ApiProfile, request: HttpRequest) -> HttpRequest {
    let credential = runtime.credential(api, &request).await.unwrap();
    ApiRuntime::preview(&credential, request).unwrap()
}

#[tokio::test]
async fn preview_signs_with_placeholders_and_stands_the_token_in() {
    let mut aws = MockAwsProfileCredentials::new();
    aws.expect_load_profile().returning(|_| Ok(dev_profile()));
    aws.expect_assume_role().times(0);
    let mut config = config_with_aws_profile(
        "base_url = \"https://abc.lambda-url.eu-west-1.on.aws\"\naws_profile = \"dev\"\n",
    );
    config.api.insert(
        "public".into(),
        toml::from_str("base_url = \"https://example.com\"\n").unwrap(),
    );
    // The preview reads the source out of the runtime's own configuration,
    // so the OAuth source and its API live in the same file as the rest.
    let oauth_config = config_with_client_credentials();
    config.auth.extend(oauth_config.auth);
    config.api.extend(oauth_config.api);
    config.auth.insert(
        "issued".into(),
        toml::from_str::<AuthToml>(
            "kind = \"token\"\ntoken = \"op://Agent/Example/credential\"\nheader = \"xi-api-key\"\nformat = \"{token}\"\n",
        )
        .unwrap(),
    );
    config.api.insert(
        "issued".into(),
        toml::from_str("base_url = \"https://api.example.com\"\n").unwrap(),
    );
    // The sections are typed the first time one is read, so every insert
    // comes first.
    let signed_api = config.api_profile("apigw").cloned().unwrap();
    let public_api = config.api_profile("public").cloned().unwrap();
    let oauth_api = config.api_profile("svc").cloned().unwrap();
    let issued_api = config.api_profile("issued").cloned().unwrap();
    let runtime = ApiRuntime::test(
        config,
        MockHttpClient::new(),
        MockTokenStore::new(),
        MockSecretResolver::new(),
        aws,
        false,
    );
    let signed = preview(
        &runtime,
        &signed_api,
        HttpRequest::new("POST", "https://abc.lambda-url.eu-west-1.on.aws/")
            .with_body(b"{}".to_vec()),
    )
    .await;
    let scope = credential_scope(&signed);
    assert!(
        scope.starts_with("<access-key-id>/") && scope.ends_with("/eu-west-1/lambda/aws4_request"),
        "{scope}"
    );
    assert_eq!(
        signed.header("x-amz-security-token"),
        Some("<session-token>")
    );
    assert_eq!(signed.body.as_deref(), Some(&b"{}"[..]));
    let bearer = preview(
        &runtime,
        &oauth_api,
        HttpRequest::new("GET", "https://api.example.com/items"),
    )
    .await;
    assert_eq!(bearer.header("authorization"), Some("Bearer <token>"));
    // A `kind = "token"` source previews the header it configures, with the
    // same placeholder, and reads no secret to do it.
    let issued = preview(
        &runtime,
        &issued_api,
        HttpRequest::new("GET", "https://api.example.com/v1/voices"),
    )
    .await;
    assert_eq!(issued.header("xi-api-key"), Some("<token>"));
    assert_eq!(issued.header("authorization"), None);
    let masked = |api| {
        let runtime = &runtime;
        async move {
            let request = HttpRequest::new("GET", "https://api.example.com/");
            let credential = runtime.credential(api, &request).await.unwrap();
            credential
                .extra_secret_headers()
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(masked(&issued_api).await, ["xi-api-key"]);
    assert!(masked(&oauth_api).await.is_empty());
    assert!(masked(&public_api).await.is_empty());
    let plain = preview(
        &runtime,
        &public_api,
        HttpRequest::new("GET", "https://example.com/"),
    )
    .await;
    assert!(plain.headers.is_empty());
}

/// A `-H` the caller wrote in the same header is replaced, not doubled: two
/// values of one credential header is a request the API reads as neither.
#[test]
fn a_configured_credential_header_replaces_one_the_caller_wrote() {
    let source = match toml::from_str::<AuthToml>(
        "kind = \"token\"\ntoken = \"op://Agent/Example/credential\"\nheader = \"X-API-Key\"\nformat = \"token {token}\"\n",
    )
    .unwrap()
    .typed("example")
    .unwrap()
    {
        crate::domain::types::AuthSource::Token(source) => source,
        other => panic!("expected a token source, got {other:?}"),
    };
    let request = source.apply(
        HttpRequest::new("GET", "https://x").with_header("x-api-key", "old"),
        "issued",
    );
    assert_eq!(request.headers.len(), 1);
    assert_eq!(request.header("X-Api-Key"), Some("token issued"));
}

#[test]
fn bearer_replaces_an_existing_authorization_header() {
    let request = with_bearer(
        HttpRequest::new("GET", "https://x").with_header("authorization", "Basic old"),
        &OAuthToken::bearer("new"),
    );
    assert_eq!(request.headers.len(), 1);
    assert_eq!(request.header("Authorization"), Some("Bearer new"));
    assert_eq!(random_value().len(), 43);
    assert_ne!(random_value(), random_value());
}
