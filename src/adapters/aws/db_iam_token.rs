//! The password of a database user that authenticates with IAM: a SigV4
//! query-string signature the database accepts for fifteen minutes. RDS and
//! Aurora sign `connect` for one host, port and user; Aurora DSQL signs
//! `DbConnect` for the cluster endpoint alone. Nothing is sent to AWS: the
//! database checks the signature when the connection opens.

use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use std::time::Duration;

use crate::adapters::sigv4::presign_query;
use crate::domain::functions::signing_target::SigningTarget;
use crate::domain::types::Credentials;

/// The longest an RDS token lives, and what the SDKs give a DSQL one.
const LIFETIME: Duration = Duration::from_secs(900);
/// The one DSQL role that connects with its own action.
const DSQL_ADMIN: &str = "admin";
/// What the SDKs leave as it is in a token: RFC 3986's unreserved characters.
const ENCODED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Signs the token of one database user, whenever a connection opens: the
/// statement's own and, later than the token of the first would live, the one
/// that stops it. `host` and `port` are the database's own, not the local end
/// of a tunnel: the database checks the name it knows itself by.
///
/// It derives no `Debug`: the role's credentials are in it.
#[derive(Clone)]
pub struct TokenSigner {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub region: String,
    pub aurora_dsql: bool,
    pub credentials: Credentials,
}

impl TokenSigner {
    /// The service the signature names, the address the token starts with and
    /// what it asks for.
    fn request(&self) -> (&'static str, String, Vec<(&'static str, &str)>) {
        let host = self.host.to_ascii_lowercase();
        if self.aurora_dsql {
            let action = if self.username == DSQL_ADMIN {
                "DbConnectAdmin"
            } else {
                "DbConnect"
            };
            ("dsql", host, vec![("Action", action)])
        } else {
            (
                "rds-db",
                format!("{host}:{}", self.port),
                vec![("Action", "connect"), ("DBUser", &self.username)],
            )
        }
    }

    /// The token signed at `now`: the presigned URL without its scheme, byte
    /// for byte what the SDKs print, which is the form the database expects
    /// as the password.
    pub fn token(&self, now: DateTime<Utc>) -> Result<String, String> {
        let (service, address, asked) = self.request();
        let mut url = url::Url::parse(&format!("https://{address}/"))
            .map_err(|error| format!("host {:?}: {error}", self.host))?;
        url.query_pairs_mut().extend_pairs(&asked);
        let target = SigningTarget {
            service: service.to_owned(),
            region: self.region.clone(),
        };
        let signed = presign_query(&url, &self.credentials, &target, now, LIFETIME)?;
        // The SDKs end the token with the signature, after the session token.
        let (signature, signed): (Vec<_>, Vec<_>) = signed
            .iter()
            .partition(|(name, _)| name == "X-Amz-Signature");
        let query: Vec<String> = asked
            .iter()
            .map(|(name, value)| (*name, *value))
            .chain(
                signed
                    .into_iter()
                    .chain(signature)
                    .map(|(name, value)| (name.as_str(), value.as_str())),
            )
            .map(|(name, value)| format!("{name}={}", utf8_percent_encode(value, ENCODED)))
            .collect();
        // The address is written here and not read back from the URL, which
        // drops a port that is its scheme's default: a database on 443 is
        // still named with it.
        Ok(format!("{address}/?{}", query.join("&")))
    }
}

/// Known answers: what botocore's `generate_db_auth_token`,
/// `generate_db_connect_admin_auth_token` and `generate_db_connect_auth_token`
/// print with the clock held at 2015-08-30T12:36:00Z. The whole token is
/// compared as text, because its text is what the database receives: read
/// back through a URL parser, a wrong encoding decodes to the right value.
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const HOST: &str = "app.cluster-abc.ap-northeast-1.rds.amazonaws.com";
    const CLUSTER: &str = "abcdefghijklmnopqrst.dsql.us-east-1.on.aws";
    const SESSION: &str = "SESSIONTOKEN/with+chars=";
    const SIGNED_AT: &str = "X-Amz-Algorithm=AWS4-HMAC-SHA256\
        &X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fap-northeast-1%2Frds-db%2Faws4_request\
        &X-Amz-Date=20150830T123600Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host";

    fn signer(session_token: Option<&str>) -> TokenSigner {
        TokenSigner {
            host: HOST.into(),
            port: 5432,
            username: "iam_reader".into(),
            region: "ap-northeast-1".into(),
            aurora_dsql: false,
            credentials: Credentials::new(
                "AKIDEXAMPLE".into(),
                SECRET.into(),
                session_token.map(str::to_owned),
                None,
            ),
        }
    }

    fn token(signer: &TokenSigner) -> String {
        let now = Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        signer.token(now).unwrap()
    }

    #[test]
    fn db_iam_token_is_botocores_for_permanent_credentials() {
        assert_eq!(
            token(&signer(None)),
            format!(
                "{HOST}:5432/?Action=connect&DBUser=iam_reader&{SIGNED_AT}&X-Amz-Signature=\
                 92afc1e2a257292684ba754e4f97d4b5ef12972738963bae17855935bdbce8aa"
            )
        );
    }

    #[test]
    fn db_iam_token_carries_and_signs_the_session_token_of_a_role() {
        assert_eq!(
            token(&signer(Some(SESSION))),
            format!(
                "{HOST}:5432/?Action=connect&DBUser=iam_reader&{SIGNED_AT}\
                 &X-Amz-Security-Token=SESSIONTOKEN%2Fwith%2Bchars%3D&X-Amz-Signature=\
                 acab8a531257c1e1fbc4d38514258a21308309ff36b6f6c3e57b4aa5f4878bb9"
            )
        );
    }

    /// 443 is the default port of the scheme the URL is signed under, and a
    /// URL drops its default port. The token names it all the same.
    #[test]
    fn db_iam_token_names_the_port_even_when_a_url_would_drop_it() {
        let signer = TokenSigner {
            port: 443,
            ..signer(None)
        };
        assert_eq!(
            token(&signer),
            format!(
                "{HOST}:443/?Action=connect&DBUser=iam_reader&{SIGNED_AT}&X-Amz-Signature=\
                 646e91d6357c4f537e9062fad3ca86d6a1d441d2ed46b049659ae45cfbc2a252"
            )
        );
    }

    #[test]
    fn db_iam_token_encodes_a_user_the_way_the_sdks_do() {
        let signer = TokenSigner {
            username: "user@example.com".into(),
            ..signer(None)
        };
        assert_eq!(
            token(&signer),
            format!(
                "{HOST}:5432/?Action=connect&DBUser=user%40example.com&{SIGNED_AT}\
                 &X-Amz-Signature=\
                 19292f741b3bbb9b6a21c9c85053518bac50abbfa2d80f8d052df4af3f02c7cf"
            )
        );
    }

    #[test]
    fn db_iam_token_is_for_one_host_port_user_and_region() {
        let signature = |signer: &TokenSigner| {
            let token = token(signer);
            token.rsplit_once("X-Amz-Signature=").unwrap().1.to_owned()
        };
        let base = signature(&signer(None));
        for other in [
            TokenSigner {
                host: "other.ap-northeast-1.rds.amazonaws.com".into(),
                ..signer(None)
            },
            TokenSigner {
                port: 3306,
                ..signer(None)
            },
            TokenSigner {
                username: "writer".into(),
                ..signer(None)
            },
            TokenSigner {
                region: "us-east-1".into(),
                ..signer(None)
            },
        ] {
            assert_ne!(signature(&other), base);
        }
    }

    #[test]
    fn db_iam_token_for_dsql_is_botocores_and_tells_admin_from_a_role() {
        for (username, action, signature) in [
            (
                "admin",
                "DbConnectAdmin",
                "83652f37150ec890e4fb4e347d735e0d6c027070a8d6375806981dfda82c1030",
            ),
            (
                "reader",
                "DbConnect",
                "03c6b045d82fdda5288d9390e3ae313181e7a3bd4b4c9d84376c041395920c4e",
            ),
        ] {
            let signer = TokenSigner {
                host: CLUSTER.into(),
                username: username.into(),
                region: "us-east-1".into(),
                aurora_dsql: true,
                ..signer(Some(SESSION))
            };
            // No port and no DBUser: the cluster endpoint alone.
            assert_eq!(
                token(&signer),
                format!(
                    "{CLUSTER}/?Action={action}&X-Amz-Algorithm=AWS4-HMAC-SHA256\
                     &X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Fus-east-1%2Fdsql%2Faws4_request\
                     &X-Amz-Date=20150830T123600Z&X-Amz-Expires=900&X-Amz-SignedHeaders=host\
                     &X-Amz-Security-Token=SESSIONTOKEN%2Fwith%2Bchars%3D\
                     &X-Amz-Signature={signature}"
                )
            );
        }
    }

    #[test]
    fn db_iam_token_refuses_a_host_no_url_can_name() {
        let signer = TokenSigner {
            host: "not a host".into(),
            ..signer(None)
        };
        let error = signer.token(Utc::now()).unwrap_err();
        assert!(error.contains("not a host"), "{error}");
    }
}
