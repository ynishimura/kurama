//! Where a secret in configuration is read from: a `<scheme>://` reference or
//! the value itself, parsed once when the file is read.

/// The AWS service an `aws-*://` reference reads from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AwsSecretStore {
    /// Secrets Manager `GetSecretValue`; a JSON `SecretString` can be indexed
    /// with `#<key>`.
    SecretsManager,
    /// Parameter Store `GetParameter` with `WithDecryption=true`.
    ParameterStore,
}

impl AwsSecretStore {
    /// The scheme that names this store in a reference.
    pub const fn scheme(self) -> &'static str {
        match self {
            Self::SecretsManager => "aws-secrets",
            Self::ParameterStore => "aws-ssm",
        }
    }

    /// The API call that reads a secret from this store, as IAM names it.
    pub const fn read_action(self) -> &'static str {
        match self {
            Self::SecretsManager => "secretsmanager:GetSecretValue",
            Self::ParameterStore => "ssm:GetParameter",
        }
    }

    /// The error code with which this store says no such name exists. Both
    /// stores also answer it for a name the role may not see, so it names
    /// the name first without ruling out the permission.
    pub const fn not_found_code(self) -> &'static str {
        match self {
            Self::SecretsManager => "ResourceNotFoundException",
            Self::ParameterStore => "ParameterNotFound",
        }
    }

    /// What the role has to be allowed to do. A refusal names it, because the
    /// service says only that access was denied.
    pub fn iam_actions(self) -> &'static str {
        match self {
            Self::SecretsManager => {
                "secretsmanager:GetSecretValue, and kms:Decrypt when the secret uses a customer-managed key"
            }
            Self::ParameterStore => "ssm:GetParameter, and kms:Decrypt for a SecureString",
        }
    }
}

/// A secret AWS holds, as one reference string names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AwsSecretRef {
    pub store: AwsSecretStore,
    /// The `~/.aws/config` profile whose role reads it, through the same
    /// AssumeRole path as `kurama env`.
    pub aws_profile: String,
    /// The secret name or ARN; a parameter name always starts with `/`.
    pub id: String,
    /// The region the reference names, from the ARN or from `?region=`. The
    /// AWS profile's region stands in when it names none.
    pub region: Option<String>,
    /// The top-level key of a JSON `SecretString`; the whole string when
    /// absent. Secrets Manager only.
    pub json_key: Option<String>,
}

impl AwsSecretRef {
    /// Where to read it, between what the reference says and what the AWS
    /// profile does. `None` is a reference nobody can resolve.
    pub fn region_in(&self, profile_region: Option<&str>) -> Option<String> {
        self.region
            .clone()
            .or_else(|| profile_region.map(str::to_owned))
    }

    /// The reference as it is written. A parameter name keeps the leading `/`
    /// internally and is written without it, so what a person wrote parses
    /// back to the same value.
    fn write(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = match self.store {
            AwsSecretStore::ParameterStore => self.id.trim_start_matches('/'),
            AwsSecretStore::SecretsManager => &self.id,
        };
        write!(f, "{}://{}/{id}", self.store.scheme(), self.aws_profile)?;
        // An ARN already says where it is, so writing `?region=` for it would
        // add an option the reference never carried.
        if let Some(region) = self
            .region
            .as_deref()
            .filter(|region| arn_region(&self.id) != Some(region))
        {
            write!(f, "?region={region}")?;
        }
        if let Some(key) = &self.json_key {
            write!(f, "#{key}")?;
        }
        Ok(())
    }
}

impl std::fmt::Display for AwsSecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.write(f)
    }
}

/// A secret: a reference resolved when it is needed, or the value itself.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// `op://<vault>/<item>/<field>`, read through the 1Password CLI.
    OnePassword(String),
    /// `aws-secrets://` or `aws-ssm://`, read with the role of an AWS profile.
    /// Boxed because this variant is five fields wide and a `SecretRef` sits
    /// inside every configured source, which the workflow carries by value.
    Aws(Box<AwsSecretRef>),
    Literal(String),
}

/// Reads what follows `<scheme>://`, given the whole value too.
type SchemeParser = fn(value: &str, rest: &str) -> Result<SecretRef, String>;

/// Every scheme a reference can name. `SecretRef::parse` dispatches through
/// this table, so the error for an unknown scheme and `kurama inventory`
/// name exactly what parses.
const SCHEMES: [(&str, SchemeParser); 3] = [
    ("op", |value, _| {
        Ok(SecretRef::OnePassword(value.to_owned()))
    }),
    (AwsSecretStore::SecretsManager.scheme(), |_, rest| {
        aws(AwsSecretStore::SecretsManager, rest)
    }),
    (AwsSecretStore::ParameterStore.scheme(), |_, rest| {
        aws(AwsSecretStore::ParameterStore, rest)
    }),
];

impl SecretRef {
    /// The schemes `parse` reads as a reference.
    pub fn schemes() -> impl Iterator<Item = &'static str> {
        SCHEMES.iter().map(|(scheme, _)| *scheme)
    }

    /// A configured value: a known scheme is a reference, anything that is not
    /// shaped like a scheme is the value itself, and a scheme kurama does not
    /// know is an error -- otherwise `aws-secret://` (an `s` short of the real
    /// one) would pass silently as a literal password.
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some(scheme) = scheme_of(value) else {
            return Ok(Self::Literal(value.to_owned()));
        };
        let rest = &value[scheme.len() + "://".len()..];
        match SCHEMES.iter().find(|(known, _)| *known == scheme) {
            Some((_, parser)) => parser(value, rest),
            None => {
                let known: Vec<String> =
                    Self::schemes().map(|known| format!("{known}://")).collect();
                Err(format!(
                    "unknown secret reference scheme \"{scheme}://\"; kurama reads {}",
                    known.join(", ")
                ))
            }
        }
    }

    /// Whether the value is a reference rather than the secret itself. A
    /// `[db.*]` password has to be one: a literal lives in the file and in
    /// every backup of it.
    pub fn is_reference(&self) -> bool {
        !matches!(self, Self::Literal(_))
    }
}

fn aws(store: AwsSecretStore, rest: &str) -> Result<SecretRef, String> {
    parse_aws(store, rest).map(|reference| SecretRef::Aws(Box::new(reference)))
}

/// The scheme of `<scheme>://...`, when the value is shaped like that. A
/// scheme is RFC 3986's: a letter, then letters, digits, `+`, `-` and `.`.
fn scheme_of(value: &str) -> Option<&str> {
    let (scheme, _) = value.split_once("://")?;
    let mut characters = scheme.chars();
    if !characters.next()?.is_ascii_alphabetic() {
        return None;
    }
    characters
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .then_some(scheme)
}

/// `<aws-profile>/<id>[?region=<region>][#<json-key>]`.
fn parse_aws(store: AwsSecretStore, rest: &str) -> Result<AwsSecretRef, String> {
    let scheme = store.scheme();
    let (head, json_key) = match rest.split_once('#') {
        Some(_) if store == AwsSecretStore::ParameterStore => {
            return Err(format!(
                "{scheme}:// reads one parameter and has no #<json-key>; \
                 use aws-secrets:// for a JSON secret"
            ));
        }
        Some((_, "")) => {
            return Err(format!("{scheme}://: the #<json-key> must not be empty"));
        }
        Some((head, key)) => (head, Some(key.to_owned())),
        None => (rest, None),
    };
    let (locator, query_region) = match head.split_once('?') {
        Some((locator, query)) => (locator, Some(parse_region_query(scheme, query)?)),
        None => (head, None),
    };
    let (aws_profile, id) = locator.split_once('/').ok_or_else(|| {
        format!("{scheme}:// needs an AWS profile and an id: {scheme}://<aws-profile>/<id>")
    })?;
    if aws_profile.is_empty() {
        return Err(format!("{scheme}://: the AWS profile must not be empty"));
    }
    if id.is_empty() {
        return Err(format!(
            "{scheme}://: the id after the AWS profile is empty"
        ));
    }
    // An ARN carries the region; a reference that also names one has to agree,
    // because there is no telling which of the two the writer meant.
    let region = match (arn_region(id), query_region) {
        (Some(arn), Some(query)) if arn != query => {
            return Err(format!(
                "{scheme}://: the ARN is in {arn} and ?region={query} says otherwise"
            ));
        }
        (Some(arn), _) => Some(arn.to_owned()),
        (None, query) => query.map(str::to_owned),
    };
    let id = match store {
        // Parameter Store names every parameter from the root; a reference may
        // leave the leading `/` out, because the scheme's own `/` is right
        // before it. An ARN is already whole, and a `/` in front of one names
        // no parameter at all -- which is what the id is asked, rather than
        // whether it carries a region, so an ARN without one is still an ARN.
        // Every leading `/` goes, because `write` strips every leading `/`,
        // and a form that does not parse back to itself is a reference an
        // error line names and kurama cannot read.
        AwsSecretStore::ParameterStore if !id.starts_with("arn:") => {
            let name = id.trim_start_matches('/');
            if name.is_empty() {
                return Err(format!(
                    "{scheme}://: the id after the AWS profile is empty"
                ));
            }
            format!("/{name}")
        }
        _ => id.to_owned(),
    };
    Ok(AwsSecretRef {
        store,
        aws_profile: aws_profile.to_owned(),
        id,
        region,
        json_key,
    })
}

/// The query of a reference: `region=<region>` and nothing else.
fn parse_region_query<'a>(scheme: &str, query: &'a str) -> Result<&'a str, String> {
    match query.split_once('=') {
        // `&` is what separates pairs, so it cannot be inside the value: a
        // query whose first pair is `region=` is not a query that says only
        // `region=`, and the difference is a region nobody wrote reaching the
        // SDK instead of a CONFIG_INVALID with its line.
        Some(("region", region)) if !region.is_empty() && !region.contains('&') => Ok(region),
        _ => Err(format!(
            "{scheme}://: the only option is ?region=<region>, got \"?{query}\""
        )),
    }
}

/// The region field of an ARN (`arn:<partition>:<service>:<region>:...`), when
/// the id is one and names a region.
fn arn_region(id: &str) -> Option<&str> {
    let mut fields = id.strip_prefix("arn:")?.split(':');
    let (_partition, _service) = (fields.next()?, fields.next()?);
    fields.next().filter(|region| !region.is_empty())
}

impl std::fmt::Debug for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnePassword(reference) => write!(f, "OnePassword({reference})"),
            Self::Aws(reference) => write!(f, "Aws({reference})"),
            Self::Literal(_) => f.write_str("Literal([REDACTED])"),
        }
    }
}

/// In configuration a secret is one string: a reference or the literal value.
/// Parsing at deserialization keeps a literal out of every `Debug` of the
/// configuration, and makes a misspelled scheme a `CONFIG_INVALID` with its
/// line instead of a password nobody notices.
impl<'de> serde::Deserialize<'de> for SecretRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for SecretRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::OnePassword(value) => serializer.serialize_str(value),
            Self::Aws(reference) => serializer.collect_str(reference),
            // A literal is the secret itself. `Debug` redacts it because it
            // must not be printed; writing it out would print it, so writing
            // it out is refused rather than quietly undoing the redaction
            // three lines above.
            Self::Literal(_) => Err(serde::ser::Error::custom(
                "a literal secret is never written out; keep it in op://, aws-secrets:// or aws-ssm://",
            )),
        }
    }
}

/// Why a secret could not be read. The reader knows what happened; this is
/// what decides the error code and the exit code, so the shell does not have
/// to read a message to tell a refusal from an unreachable service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretFailure {
    /// A person has to act: 1Password is not signed in, or the biometric
    /// prompt nobody answered was killed.
    NeedsAPerson,
    /// The reference names something that cannot be used, or the value is not
    /// what the reference says it is (binary, not JSON, no such key).
    Invalid,
    /// The service answered and refused; `not_found` when its code was the
    /// store's `not_found_code`.
    Rejected {
        store: AwsSecretStore,
        not_found: bool,
    },
    /// The service could not be reached, or the SDK failed.
    Unreachable,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(value: &str) -> AwsSecretRef {
        match SecretRef::parse(value).expect(value) {
            SecretRef::Aws(reference) => *reference,
            other => panic!("{value} parsed as {other:?}"),
        }
    }

    #[test]
    fn secret_reference_schemes_name_their_store_and_anything_else_is_the_value() {
        assert_eq!(
            SecretRef::parse("op://Agent/GitHub/secret").unwrap(),
            SecretRef::OnePassword("op://Agent/GitHub/secret".into())
        );
        assert_eq!(
            SecretRef::parse("hunter2").unwrap(),
            SecretRef::Literal("hunter2".into())
        );
        assert_eq!(
            parsed("aws-secrets://dev/app/db").store,
            AwsSecretStore::SecretsManager
        );
        assert_eq!(
            parsed("aws-ssm://dev/app/db").store,
            AwsSecretStore::ParameterStore
        );
        assert!(SecretRef::parse("op://x/y/z").unwrap().is_reference());
        assert!(SecretRef::parse("aws-ssm://dev/p").unwrap().is_reference());
        assert!(!SecretRef::parse("plain").unwrap().is_reference());
    }

    /// The table `parse` dispatches through is what the unknown-scheme error
    /// names and what `kurama inventory` lists, so a scheme added to it is in
    /// all three.
    #[test]
    fn the_unknown_scheme_error_names_every_scheme_parse_reads() {
        let error = SecretRef::parse("bogus://x").unwrap_err();
        let schemes: Vec<&str> = SecretRef::schemes().collect();
        assert_eq!(schemes, ["op", "aws-secrets", "aws-ssm"]);
        for scheme in schemes {
            assert!(error.contains(&format!(" {scheme}://")), "{error}");
            let reference = format!("{scheme}://profile/item");
            assert!(
                SecretRef::parse(&reference)
                    .expect(&reference)
                    .is_reference()
            );
        }
    }

    /// A misspelled scheme used to become a literal password: the value went
    /// to the database as the password and nothing said why it failed.
    #[test]
    fn secret_reference_with_an_unknown_scheme_is_an_error_and_never_a_literal() {
        for unknown in [
            "aws-secret://dev/app/db",
            "aws-ssms://dev/app/db",
            "vault://secret/app",
            "OP://Agent/item/field",
        ] {
            let error = SecretRef::parse(unknown).expect_err(unknown);
            assert!(error.contains("unknown secret reference scheme"), "{error}");
        }
        // Only a value shaped like a scheme is held to that rule.
        for literal in ["hunter2", "a/b/c", "1://x", "-op://x", "://x", "op:/x"] {
            assert_eq!(
                SecretRef::parse(literal).unwrap(),
                SecretRef::Literal(literal.into()),
                "{literal}"
            );
        }
    }

    #[test]
    fn an_aws_reference_carries_the_profile_the_id_the_region_and_the_json_key() {
        let plain = parsed("aws-secrets://dev/rds!db-1234");
        assert_eq!(plain.aws_profile, "dev");
        assert_eq!(plain.id, "rds!db-1234");
        assert_eq!(plain.region, None);
        assert_eq!(plain.json_key, None);
        // A name may contain slashes, and the first one still ends the profile.
        assert_eq!(parsed("aws-secrets://dev/app/prod/db").id, "app/prod/db");
        let keyed = parsed("aws-secrets://dev/app/db?region=us-west-2#password");
        assert_eq!(keyed.id, "app/db");
        assert_eq!(keyed.region.as_deref(), Some("us-west-2"));
        assert_eq!(keyed.json_key.as_deref(), Some("password"));
        // A parameter is named from the root whether the reference says so.
        assert_eq!(
            parsed("aws-ssm://dev/app/db-password").id,
            "/app/db-password"
        );
        assert_eq!(
            parsed("aws-ssm://dev//app/db-password").id,
            "/app/db-password"
        );
        // `write` strips every leading `/`, so parse puts back exactly one.
        assert_eq!(parsed("aws-ssm://dev///app/db").id, "/app/db");
        // An ARN is an ARN whether or not it names a region, and a `/` in
        // front of one names no parameter at all.
        for arn in [
            "arn:aws:ssm:us-east-1:123456789012:parameter/app/db",
            "arn:aws:ssm::123456789012:parameter/app/db",
        ] {
            assert_eq!(parsed(&format!("aws-ssm://dev/{arn}")).id, arn, "{arn}");
        }
    }

    #[test]
    fn a_region_comes_from_the_arn_then_the_option_then_the_aws_profile() {
        const ARN: &str = "arn:aws:secretsmanager:ap-northeast-1:123456789012:secret:app/db-AbCdEf";
        let from_arn = parsed(&format!("aws-secrets://dev/{ARN}"));
        assert_eq!(from_arn.region.as_deref(), Some("ap-northeast-1"));
        assert_eq!(
            from_arn.region_in(Some("eu-west-1")).as_deref(),
            Some("ap-northeast-1")
        );
        let from_option = parsed("aws-ssm://dev/app/db?region=us-east-1");
        assert_eq!(
            from_option.region_in(Some("eu-west-1")).as_deref(),
            Some("us-east-1")
        );
        let from_profile = parsed("aws-ssm://dev/app/db");
        assert_eq!(
            from_profile.region_in(Some("eu-west-1")).as_deref(),
            Some("eu-west-1")
        );
        assert_eq!(from_profile.region_in(None), None);
        // The same region twice is not a contradiction.
        assert_eq!(
            parsed(&format!("aws-secrets://dev/{ARN}?region=ap-northeast-1"))
                .region
                .as_deref(),
            Some("ap-northeast-1")
        );
    }

    #[test]
    fn an_aws_reference_nobody_can_resolve_is_an_error() {
        for (invalid, says) in [
            // The ARN and the option cannot both decide.
            (
                "aws-secrets://dev/arn:aws:secretsmanager:ap-northeast-1:1:secret:a?region=us-east-1",
                "says otherwise",
            ),
            ("aws-secrets://dev/app/db?vault=x", "?region=<region>"),
            ("aws-secrets://dev/app/db?region=", "?region=<region>"),
            // `region=` first does not make the rest of the query disappear.
            // Versions and stages are not implemented, so this is what someone
            // reaching for `AWSCURRENT` writes.
            (
                "aws-secrets://dev/app/db?region=us-east-1&version=AWSCURRENT",
                "?region=<region>",
            ),
            (
                "aws-ssm://dev/app/db?region=eu-west-1&x=1",
                "?region=<region>",
            ),
            // A parameter named from the root and nothing else is no parameter.
            ("aws-ssm://dev//", "id after the AWS profile is empty"),
            ("aws-secrets://dev", "needs an AWS profile and an id"),
            ("aws-secrets:///app/db", "profile must not be empty"),
            ("aws-secrets://dev/", "id after the AWS profile is empty"),
            ("aws-secrets://dev/app/db#", "must not be empty"),
            // One parameter is one value: there is no JSON key to take.
            ("aws-ssm://dev/app/db#password", "has no #<json-key>"),
        ] {
            let error = SecretRef::parse(invalid).expect_err(invalid);
            assert!(error.contains(says), "{invalid}: {error}");
        }
    }

    /// The reference is what `Debug`, the configuration and an error line
    /// carry, so writing it out has to produce the value it parsed from.
    #[test]
    fn every_reference_form_writes_back_to_itself() {
        for reference in [
            "aws-secrets://dev/app/db",
            "aws-secrets://dev/app/db#password",
            "aws-secrets://dev/app/db?region=us-west-2",
            "aws-secrets://dev/app/db?region=us-west-2#username",
            "aws-secrets://dev/arn:aws:secretsmanager:ap-northeast-1:1:secret:a-Xy#password",
            "aws-ssm://dev/app/db-password",
            "aws-ssm://work/app/db?region=eu-west-1",
            "aws-ssm://dev/arn:aws:ssm:us-east-1:123456789012:parameter/app/db",
        ] {
            let written = parsed(reference);
            assert_eq!(written.to_string(), reference, "{reference}");
        }
        // The forms `write` deliberately shortens. Writing them out produces a
        // different string, and that string still has to parse back to the
        // same value -- which is the half the identity forms above cannot
        // check, because for them the check is the line before it.
        for (reference, printed) in [
            (
                "aws-secrets://dev/arn:aws:secretsmanager:ap-northeast-1:1:secret:a-Xy?region=ap-northeast-1",
                "aws-secrets://dev/arn:aws:secretsmanager:ap-northeast-1:1:secret:a-Xy",
            ),
            ("aws-ssm://dev//app/db", "aws-ssm://dev/app/db"),
            ("aws-ssm://dev///app/db", "aws-ssm://dev/app/db"),
        ] {
            let written = parsed(reference);
            assert_eq!(written.to_string(), printed, "{reference}");
            assert_eq!(parsed(printed), written, "{reference}");
        }
    }

    /// Every combination the grammar allows, rather than a list someone has to
    /// remember to extend. The forms a hand-written corpus leaves out are
    /// exactly the interesting ones -- a form `write` shortens, a form parse
    /// normalizes -- and each has to come back as the same value, and print
    /// the same way the second time.
    #[test]
    fn every_combination_of_the_grammar_survives_a_round_trip() {
        const PROFILES: [&str; 2] = ["dev", "work-2"];
        const SECRET_IDS: [&str; 4] = [
            "app/db",
            "rds!cluster-abc123",
            "arn:aws:secretsmanager:ap-northeast-1:1:secret:app/db-AbCdEf",
            // An ARN is an ARN even where the region field is empty.
            "arn:aws:secretsmanager::1:secret:app/db-AbCdEf",
        ];
        const PARAMETER_IDS: [&str; 4] = [
            "app/db",
            "/app/db",
            "//app/db",
            "arn:aws:ssm:us-east-1:1:parameter/app/db",
        ];
        const REGIONS: [&str; 3] = ["", "?region=eu-west-1", "?region=ap-northeast-1"];

        let mut accepted = 0;
        for profile in PROFILES {
            for (scheme, ids, keys) in [
                (
                    "aws-secrets",
                    SECRET_IDS.as_slice(),
                    ["", "#password"].as_slice(),
                ),
                // One parameter is one value: there is no key to take.
                ("aws-ssm", PARAMETER_IDS.as_slice(), [""].as_slice()),
            ] {
                for id in ids {
                    for region in REGIONS {
                        for key in keys {
                            let written = format!("{scheme}://{profile}/{id}{region}{key}");
                            // A rejected combination is the grammar refusing a
                            // contradiction, which other tests pin by name.
                            let Ok(SecretRef::Aws(value)) = SecretRef::parse(&written) else {
                                continue;
                            };
                            accepted += 1;
                            let printed = value.to_string();
                            let reparsed = match SecretRef::parse(&printed) {
                                Ok(SecretRef::Aws(again)) => again,
                                other => panic!("{written} wrote {printed}, which is {other:?}"),
                            };
                            assert_eq!(value, reparsed, "{written} wrote {printed}");
                            assert_eq!(reparsed.to_string(), printed, "{written}");
                        }
                    }
                }
            }
        }
        // A generator that accepts nothing verifies nothing.
        assert!(accepted >= 50, "only {accepted} combinations were accepted");
    }

    /// `Debug` redacts a literal because it must not be printed. Writing it
    /// out is printing it, so the one path that could undo that redaction
    /// refuses instead of quietly carrying the password into whatever reads
    /// the configuration back.
    #[test]
    fn a_reference_writes_itself_and_a_literal_is_never_written_out() {
        assert_eq!(
            serde_json::to_string(&SecretRef::OnePassword("op://Agent/item/field".into())).unwrap(),
            "\"op://Agent/item/field\""
        );
        assert_eq!(
            serde_json::to_string(&SecretRef::parse("aws-ssm://dev/app/db").unwrap()).unwrap(),
            "\"aws-ssm://dev/app/db\""
        );
        let refused = serde_json::to_string(&SecretRef::Literal("hunter2".into()))
            .expect_err("a literal is not written out");
        assert!(!refused.to_string().contains("hunter2"), "{refused}");
    }

    #[test]
    fn secret_reference_debug_shows_the_reference_and_hides_a_literal() {
        assert_eq!(
            format!(
                "{:?}",
                SecretRef::parse("op://Agent/GitHub/secret").unwrap()
            ),
            "OnePassword(op://Agent/GitHub/secret)"
        );
        assert_eq!(
            format!(
                "{:?}",
                SecretRef::parse("aws-secrets://dev/app/db#password").unwrap()
            ),
            "Aws(aws-secrets://dev/app/db#password)"
        );
        assert_eq!(
            format!("{:?}", SecretRef::parse("plain-secret").unwrap()),
            "Literal([REDACTED])"
        );
    }

    #[test]
    fn each_store_names_the_actions_a_refusal_needs() {
        assert!(
            AwsSecretStore::SecretsManager
                .iam_actions()
                .contains("secretsmanager:GetSecretValue")
        );
        assert!(
            AwsSecretStore::ParameterStore
                .iam_actions()
                .contains("ssm:GetParameter")
        );
        for store in [
            AwsSecretStore::SecretsManager,
            AwsSecretStore::ParameterStore,
        ] {
            assert!(store.iam_actions().contains("kms:Decrypt"), "{store:?}");
        }
    }
}
