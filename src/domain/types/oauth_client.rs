//! One `[auth.<name>]` credential source of kind `oauth`, validated.

use serde::{Deserialize, Serialize};

use super::SecretRef;

/// The OAuth 2.0 grant a credential source uses to get its first token.
/// Read off `grant_type` by serde, so an unknown value is refused with the
/// line it sits on and the values it could have been.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// Authorization code with PKCE through the browser and a loopback redirect.
    AuthorizationCode,
    /// Device authorization: the person enters a code on another device.
    DeviceCode,
    /// Client credentials: no person involved.
    ClientCredentials,
}

impl GrantType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationCode => "authorization_code",
            Self::DeviceCode => "device_code",
            Self::ClientCredentials => "client_credentials",
        }
    }

    /// Whether getting a token needs a person (a browser or a code entry).
    pub fn needs_human(self) -> bool {
        !matches!(self, Self::ClientCredentials)
    }
}

/// The endpoints a grant talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthEndpoints {
    pub auth_url: Option<String>,
    pub token_url: String,
    pub device_auth_url: Option<String>,
}

impl OAuthEndpoints {
    /// The endpoint the grant starts with, or what is missing for it.
    pub fn check_for(&self, grant: GrantType) -> Result<(), String> {
        match grant {
            GrantType::AuthorizationCode if self.auth_url.is_none() => {
                Err("authorization_code needs auth_url (or issuer)".into())
            }
            GrantType::DeviceCode if self.device_auth_url.is_none() => {
                Err("device_code needs device_auth_url (or issuer)".into())
            }
            _ => Ok(()),
        }
    }
}

/// Where the endpoints come from: OpenID Connect discovery on the issuer, or
/// URLs written in the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSource {
    Issuer(String),
    Explicit(OAuthEndpoints),
}

/// Default variable `kurama env` exports the token into.
pub const DEFAULT_TOKEN_ENV_VAR: &str = "KURAMA_TOKEN";

/// A validated `[auth.<name>]` entry of kind `oauth`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientConfig {
    pub name: String,
    pub grant_type: GrantType,
    pub endpoints: EndpointSource,
    pub client_id: String,
    pub client_secret: Option<SecretRef>,
    pub scopes: Vec<String>,
    /// Variable `kurama env` / `kurama exec` put the access token in.
    pub env_var: String,
    /// Fixed loopback port for the authorization code redirect; an ephemeral
    /// port when absent.
    pub redirect_port: Option<u16>,
}

impl OAuthClientConfig {
    /// The rules a valid entry follows; the message names the offending key.
    pub fn validate(&self) -> Result<(), String> {
        if self.client_id.is_empty() {
            return Err("client_id must not be empty".into());
        }
        match &self.endpoints {
            EndpointSource::Issuer(issuer) => check_credential_url("issuer", issuer),
            EndpointSource::Explicit(endpoints) => {
                endpoints.check_for(self.grant_type)?;
                check_credential_url("token_url", &endpoints.token_url)?;
                if let Some(url) = &endpoints.auth_url {
                    check_credential_url("auth_url", url)?;
                }
                if let Some(url) = &endpoints.device_auth_url {
                    check_credential_url("device_auth_url", url)?;
                }
                Ok(())
            }
        }
    }
}

/// `value` is an authorization server URL kurama may send grant credentials
/// to (a client secret, a code, a refresh token, a device code): `https://`,
/// or `http://` on a loopback host -- `localhost`, `127.0.0.0/8` or `::1` --
/// so local fakes and development servers work. Plain HTTP to any other host
/// would carry those credentials in the clear.
pub fn check_credential_url(key: &str, value: &str) -> Result<(), String> {
    check_http_url(key, value)?;
    let url = url::Url::parse(value).expect("check_http_url parsed it");
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() == "https" || loopback {
        return Ok(());
    }
    Err(format!(
        "{key} must be an https URL (http is allowed only for localhost or a loopback address), got '{value}'"
    ))
}

/// `value` is an `http://` or `https://` URL. `http://` is allowed so local
/// fakes and development servers work; the scheme is still checked so a bare
/// host name fails early.
pub fn check_http_url(key: &str, value: &str) -> Result<(), String> {
    match url::Url::parse(value) {
        Ok(url) if url.scheme() == "https" || url.scheme() == "http" => Ok(()),
        Ok(url) => Err(format!(
            "{key} must be an http(s) URL, got scheme '{}'",
            url.scheme()
        )),
        Err(error) => Err(format!("{key} is not a URL ({error})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn explicit(grant: GrantType, endpoints: OAuthEndpoints) -> OAuthClientConfig {
        OAuthClientConfig {
            name: "github".into(),
            grant_type: grant,
            endpoints: EndpointSource::Explicit(endpoints),
            client_id: "id".into(),
            client_secret: None,
            scopes: vec![],
            env_var: DEFAULT_TOKEN_ENV_VAR.into(),
            redirect_port: None,
        }
    }

    #[test]
    fn grant_types_parse_and_only_client_credentials_is_headless() {
        assert_eq!(
            serde_json::from_str::<GrantType>("\"device_code\"").unwrap(),
            GrantType::DeviceCode
        );
        let unknown = serde_json::from_str::<GrantType>("\"implicit\"")
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("implicit"), "{unknown}");
        assert!(unknown.contains("`client_credentials`"), "{unknown}");
        assert!(GrantType::AuthorizationCode.needs_human());
        assert!(GrantType::DeviceCode.needs_human());
        assert!(!GrantType::ClientCredentials.needs_human());
        assert_eq!(GrantType::ClientCredentials.as_str(), "client_credentials");
    }

    #[test]
    fn explicit_endpoints_must_cover_the_grant() {
        let token_only = OAuthEndpoints {
            auth_url: None,
            token_url: "https://as.example.com/token".into(),
            device_auth_url: None,
        };
        assert!(
            explicit(GrantType::ClientCredentials, token_only.clone())
                .validate()
                .is_ok()
        );
        assert!(
            explicit(GrantType::AuthorizationCode, token_only.clone())
                .validate()
                .unwrap_err()
                .contains("auth_url")
        );
        assert!(
            explicit(GrantType::DeviceCode, token_only)
                .validate()
                .unwrap_err()
                .contains("device_auth_url")
        );
    }

    #[test]
    fn urls_must_be_http_or_https() {
        let bad = OAuthEndpoints {
            auth_url: None,
            token_url: "ftp://as.example.com/token".into(),
            device_auth_url: None,
        };
        assert!(
            explicit(GrantType::ClientCredentials, bad)
                .validate()
                .unwrap_err()
                .contains("token_url")
        );
        let mut config = explicit(
            GrantType::ClientCredentials,
            OAuthEndpoints {
                auth_url: None,
                token_url: "http://127.0.0.1:8080/token".into(),
                device_auth_url: None,
            },
        );
        assert!(config.validate().is_ok());
        config.endpoints = EndpointSource::Issuer("not a url".into());
        assert!(config.validate().unwrap_err().contains("issuer"));
        config.client_id.clear();
        config.endpoints = EndpointSource::Issuer("https://accounts.example.com".into());
        assert!(config.validate().unwrap_err().contains("client_id"));
    }

    #[rstest::rstest]
    #[case("https://as.example.com/token")]
    #[case("http://127.0.0.1:8080/token")]
    #[case("http://127.9.8.7/token")]
    #[case("http://localhost:8080/token")]
    #[case("http://LOCALHOST/token")]
    #[case("http://[::1]:8080/token")]
    fn credential_urls_are_https_or_loopback_http(#[case] url: &str) {
        assert_eq!(check_credential_url("token_url", url), Ok(()));
    }

    #[rstest::rstest]
    #[case("http://as.example.com/token")]
    #[case("http://10.0.0.1/token")]
    #[case("http://[fe80::1]/token")]
    #[case("http://localhost.example.com/token")]
    #[case("http://127.0.0.1.nip.io/token")]
    fn credential_urls_refuse_remote_http(#[case] url: &str) {
        let error = check_credential_url("token_url", url).unwrap_err();
        assert!(
            error.starts_with("token_url must be an https URL") && error.contains(url),
            "{error}"
        );
    }

    #[test]
    fn every_configured_endpoint_refuses_remote_http() {
        let https = |path: &str| format!("https://as.example.com/{path}");
        let endpoints = OAuthEndpoints {
            auth_url: Some(https("authorize")),
            token_url: https("token"),
            device_auth_url: Some(https("device")),
        };
        let remote = "http://as.example.com/x".to_string();
        let mut config = explicit(GrantType::AuthorizationCode, endpoints.clone());
        assert_eq!(config.validate(), Ok(()));
        for (key, changed) in [
            (
                "token_url",
                OAuthEndpoints {
                    token_url: remote.clone(),
                    ..endpoints.clone()
                },
            ),
            (
                "auth_url",
                OAuthEndpoints {
                    auth_url: Some(remote.clone()),
                    ..endpoints.clone()
                },
            ),
            (
                "device_auth_url",
                OAuthEndpoints {
                    device_auth_url: Some(remote.clone()),
                    ..endpoints.clone()
                },
            ),
        ] {
            config.endpoints = EndpointSource::Explicit(changed);
            let error = config.validate().unwrap_err();
            assert!(error.starts_with(key), "{key}: {error}");
        }
        config.endpoints = EndpointSource::Issuer("http://accounts.example.com".into());
        assert!(config.validate().unwrap_err().starts_with("issuer"));
    }
}
