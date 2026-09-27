//! Where a SigV4 signature points: the service name and the region of a
//! request to an `[api.*]` profile with `aws_profile`. `kurama api` takes
//! the service from `--service`, then the profile's `service`, then the
//! host; the region from `--region`, the profile's `region`, the host, then
//! the AWS profile's region. Anything left open stops the command before a
//! credential is requested.

/// The service name and region a request is signed for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningTarget {
    pub service: String,
    pub region: String,
}

/// What one layer (the command line, the `[api.*]` profile, the host) says
/// about the target; `None` leaves the decision to the next layer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SigningHint {
    pub service: Option<String>,
    pub region: Option<String>,
}

impl SigningHint {
    /// `self` where it says something, `other` elsewhere.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self {
            service: self.service.or(other.service),
            region: self.region.or(other.region),
        }
    }
}

/// The target from `hint` (the command line over the profile), the host,
/// and, for the region only, the AWS profile's region. The error says what
/// is still missing and for which host.
pub fn resolve_signing_target(
    hint: &SigningHint,
    host: &str,
    aws_profile_region: Option<&str>,
) -> Result<SigningTarget, String> {
    let inferred = infer_from_host(host);
    let service = hint.service.clone().or(inferred.service);
    let region = hint
        .region
        .clone()
        .or(inferred.region)
        .or_else(|| aws_profile_region.map(str::to_string));
    match (service, region) {
        (Some(service), Some(region)) => Ok(SigningTarget { service, region }),
        (service, region) => {
            let missing = match (service.is_some(), region.is_some()) {
                (false, false) => "service and region",
                (false, true) => "service",
                _ => "region",
            };
            Err(format!(
                "the SigV4 {missing} to sign for cannot be told from host {host:?}"
            ))
        }
    }
}

/// Domains whose hostnames name the service and the region.
const AWS_DOMAINS: [&str; 3] = [".amazonaws.com.cn", ".amazonaws.com", ".on.aws"];

/// The service and region an AWS hostname names: the two labels before the
/// AWS domain are the service and the region in either order
/// (`<id>.execute-api.<region>.amazonaws.com` for API Gateway,
/// `<id>.lambda-url.<region>.on.aws` for a Lambda function URL,
/// `<domain>.<region>.es.amazonaws.com` for OpenSearch,
/// `<id>.appsync-api.<region>.amazonaws.com` for AppSync,
/// `<service>.<region>.amazonaws.com` for most services), and a global
/// endpoint (`sts.amazonaws.com`) uses us-east-1. Any other host
/// says nothing.
pub fn infer_from_host(host: &str) -> SigningHint {
    let host = host.to_ascii_lowercase();
    let Some(labels) = AWS_DOMAINS
        .iter()
        .find_map(|domain| host.strip_suffix(domain))
    else {
        return SigningHint::default();
    };
    // GovCloud's partition-global aliases need explicit signing settings.
    if labels.ends_with(".us-gov") {
        return SigningHint::default();
    }
    let mut labels = labels
        .rsplit('.')
        .filter(|label| !label.is_empty() && !["dualstack", "vpce"].contains(label));
    let (Some(last), previous) = (labels.next(), labels.next()) else {
        return SigningHint::default();
    };
    let (service, region) = if is_region(last) {
        (previous, Some(last))
    } else if previous.is_some_and(is_region) {
        (Some(last), previous)
    } else {
        (
            Some(last),
            host.ends_with(".amazonaws.com").then_some("us-east-1"),
        )
    };
    SigningHint {
        service: service.map(signing_name),
        region: region.map(str::to_string),
    }
}

/// Whether `host` is an Aurora DSQL cluster endpoint
/// (`<id>.dsql.<region>.on.aws`), the only name such a cluster is reached
/// under. It signs its IAM token differently from RDS, and it leaves out the
/// PostgreSQL functions a statement is stopped with.
pub fn names_aurora_dsql(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let Some(labels) = host.strip_suffix(".on.aws") else {
        return false;
    };
    // Exactly this shape, and not whatever `infer_from_host` reads a service
    // from: that one accepts either order and three domains, for `kurama api`.
    let labels: Vec<&str> = labels.split('.').collect();
    matches!(labels.as_slice(), [id, "dsql", region] if !id.is_empty() && is_region(region))
}

/// The signing name of a hostname label, where the two differ.
fn signing_name(label: &str) -> String {
    let label = label.strip_suffix("-fips").unwrap_or(label);
    match label {
        "lambda-url" => "lambda",
        "appsync-api" => "appsync",
        other => other,
    }
    .to_string()
}

/// `ap-northeast-1`, `us-gov-west-1`, `cn-north-1`, `us-isob-east-1`: a
/// two-letter partition prefix, one or more words, and a number.
fn is_region(label: &str) -> bool {
    let parts: Vec<&str> = label.split('-').collect();
    let [prefix, words @ .., number] = parts.as_slice() else {
        return false;
    };
    prefix.len() == 2
        && prefix.chars().all(|c| c.is_ascii_lowercase())
        && !words.is_empty()
        && words
            .iter()
            .all(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()))
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aurora_dsql_endpoint_is_told_from_every_other_database_host() {
        assert!(names_aurora_dsql(
            "abcdefghijklmnopqrst.dsql.ap-northeast-1.on.aws"
        ));
        assert!(names_aurora_dsql(
            "ABCDEFGHIJKLMNOPQRST.DSQL.us-east-1.on.aws"
        ));
        for other in [
            "app.cluster-abc.ap-northeast-1.rds.amazonaws.com",
            "dsql.internal.example.com",
            "abc.lambda-url.ap-northeast-1.on.aws",
            "localhost",
            // The control plane, another partition's domain, the labels the
            // other way round and a name with more in front: none of them is
            // a cluster, and a false yes drops the statement timeout and the
            // cancel of a server that has both.
            "dsql.us-east-1.amazonaws.com",
            "abcdefghijklmnopqrst.dsql.us-east-1.amazonaws.com",
            "abcdefghijklmnopqrst.us-east-1.dsql.on.aws",
            "abcdefghijklmnopqrst.dsql-fips.us-east-1.on.aws",
            "extra.abcdefghijklmnopqrst.dsql.us-east-1.on.aws",
            "abcdefghijklmnopqrst.dsql.nowhere.on.aws",
        ] {
            assert!(!names_aurora_dsql(other), "{other}");
        }
    }

    fn hint(service: Option<&str>, region: Option<&str>) -> SigningHint {
        SigningHint {
            service: service.map(str::to_string),
            region: region.map(str::to_string),
        }
    }

    #[rstest::rstest]
    #[case(
        "abc123.execute-api.ap-northeast-1.amazonaws.com",
        Some("execute-api"),
        Some("ap-northeast-1")
    )]
    #[case(
        "abc123.lambda-url.us-east-1.on.aws",
        Some("lambda"),
        Some("us-east-1")
    )]
    #[case(
        "search-logs-abc.ap-northeast-1.es.amazonaws.com",
        Some("es"),
        Some("ap-northeast-1")
    )]
    #[case(
        "abc.ap-northeast-1.aoss.amazonaws.com",
        Some("aoss"),
        Some("ap-northeast-1")
    )]
    #[case(
        "abc.appsync-api.eu-central-1.amazonaws.com",
        Some("appsync"),
        Some("eu-central-1")
    )]
    #[case(
        "dynamodb.us-gov-west-1.amazonaws.com",
        Some("dynamodb"),
        Some("us-gov-west-1")
    )]
    #[case("sts.cn-north-1.amazonaws.com.cn", Some("sts"), Some("cn-north-1"))]
    #[case("bucket.s3.amazonaws.com", Some("s3"), Some("us-east-1"))]
    #[case("IAM.amazonaws.com", Some("iam"), Some("us-east-1"))]
    #[case(
        "s3.dualstack.ap-northeast-1.amazonaws.com",
        Some("s3"),
        Some("ap-northeast-1")
    )]
    #[case(
        "vpce-0abc.execute-api.ap-northeast-1.vpce.amazonaws.com",
        Some("execute-api"),
        Some("ap-northeast-1")
    )]
    #[case(
        "dynamodb-fips.us-gov-west-1.amazonaws.com",
        Some("dynamodb"),
        Some("us-gov-west-1")
    )]
    #[case("iam.us-gov.amazonaws.com", None, None)]
    #[case("api.example.com", None, None)]
    #[case("amazonaws.com", None, None)]
    #[case("127.0.0.1", None, None)]
    fn hosts_name_the_service_and_the_region(
        #[case] host: &str,
        #[case] service: Option<&str>,
        #[case] region: Option<&str>,
    ) {
        assert_eq!(infer_from_host(host), hint(service, region));
    }

    #[test]
    fn the_hint_wins_over_the_host_and_the_aws_profile_region_is_the_last_resort() {
        let apigw = "abc.execute-api.ap-northeast-1.amazonaws.com";
        assert_eq!(
            resolve_signing_target(&SigningHint::default(), apigw, Some("us-east-1")).unwrap(),
            SigningTarget {
                service: "execute-api".into(),
                region: "ap-northeast-1".into()
            }
        );
        assert_eq!(
            resolve_signing_target(&hint(Some("svc"), Some("eu-west-1")), apigw, None).unwrap(),
            SigningTarget {
                service: "svc".into(),
                region: "eu-west-1".into()
            }
        );
        assert_eq!(
            resolve_signing_target(
                &hint(Some("execute-api"), None),
                "api.example.com",
                Some("us-east-1")
            )
            .unwrap(),
            SigningTarget {
                service: "execute-api".into(),
                region: "us-east-1".into()
            }
        );
        assert_eq!(
            resolve_signing_target(
                &SigningHint::default(),
                "sts.amazonaws.com",
                Some("ap-northeast-1")
            )
            .unwrap(),
            SigningTarget {
                service: "sts".into(),
                region: "us-east-1".into()
            }
        );
        assert_eq!(
            hint(Some("a"), None).or(hint(Some("b"), Some("r"))),
            hint(Some("a"), Some("r"))
        );
    }

    #[test]
    fn what_is_missing_is_named() {
        let error =
            resolve_signing_target(&SigningHint::default(), "api.example.com", None).unwrap_err();
        assert_eq!(
            error,
            "the SigV4 service and region to sign for cannot be told from host \"api.example.com\""
        );
        let error = resolve_signing_target(
            &SigningHint::default(),
            "api.example.com",
            Some("us-east-1"),
        )
        .unwrap_err();
        assert!(error.starts_with("the SigV4 service to sign"), "{error}");
        let error =
            resolve_signing_target(&hint(Some("execute-api"), None), "api.example.com", None)
                .unwrap_err();
        assert!(error.starts_with("the SigV4 region to sign"), "{error}");
    }

    #[test]
    fn region_labels() {
        for region in [
            "us-east-1",
            "ap-northeast-3",
            "us-gov-west-1",
            "us-isob-east-1",
        ] {
            assert!(is_region(region), "{region}");
        }
        for other in [
            "execute-api",
            "s3",
            "us-east",
            "east-1",
            "usa-east-1",
            "us--1",
            "us-east-",
        ] {
            assert!(!is_region(other), "{other}");
        }
    }
}
