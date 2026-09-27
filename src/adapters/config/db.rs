//! Named database connections: one `[db.*]` section per database, converted
//! once into the settings its engine actually has.
use crate::domain::functions::signing_target::names_aurora_dsql;
use crate::domain::types::SecretRef;
use crate::domain::types::database::{DbError, DbLimits, InvalidDb};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DbEngine {
    Sqlite,
    Postgresql,
    Mysql,
}

impl DbEngine {
    /// Every engine a `[db.*]` section may name. The contract publishes this,
    /// so an engine added to the enum is an engine agents can be told about.
    pub const ALL: [Self; 3] = [Self::Sqlite, Self::Postgresql, Self::Mysql];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgresql => "postgresql",
            Self::Mysql => "mysql",
        }
    }

    /// The port a server listens on when the section names none.
    fn default_port(self) -> u16 {
        match self {
            Self::Postgresql => 5432,
            Self::Mysql => 3306,
            Self::Sqlite => 0,
        }
    }
}

/// A bastion the connection is tunnelled through, because the database is not
/// reachable from here.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelSection {
    pub kind: TunnelKind,
    pub aws_profile: String,
    /// The bastion, by its `Name` tag or by its instance id; one of the two.
    pub instance_name: Option<String>,
    pub instance_id: Option<String>,
    /// Where the bastion is. The AWS profile's region when the section omits it.
    pub region: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TunnelKind {
    Ssm,
}

/// A bastion, after validation: exactly one way to name it.
#[derive(Clone, Debug)]
pub struct Tunnel {
    pub aws_profile: String,
    pub instance: InstanceRef,
    pub region: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceRef {
    Name(String),
    Id(String),
}

impl InstanceRef {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Name(value) | Self::Id(value) => value,
        }
    }
}

/// An RDS user that authenticates with IAM: the AWS profile whose role signs
/// the token that stands in for a password.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IamSection {
    pub aws_profile: String,
    /// Where the database is. The host name when it carries one, then the AWS
    /// profile's region, when the section omits it.
    pub region: Option<String>,
}

/// How a server learns who is connecting: one or the other, never both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbAuth {
    /// A reference, read when the connection is opened.
    Password(SecretRef),
    /// A token signed with the role of an AWS profile.
    Iam(IamSection),
}

/// How far the client checks the server's certificate.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DbTls {
    /// Check the chain and the host name. The default.
    #[default]
    VerifyFull,
    /// Check the chain only. For a server reached under a name its certificate
    /// does not carry, which is what an internal alias or a tunnel gives.
    VerifyCa,
    Disable,
}

/// A `[db.*]` section as it is written. `connection` turns it into the typed
/// settings of one engine, so no later code reads a field another engine owns.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DbSection {
    pub engine: DbEngine,
    /// SQLite only: the database file.
    pub path: Option<String>,
    /// Servers only.
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    /// A literal name, or a reference: `op://`, `aws-secrets://`, `aws-ssm://`.
    pub username: Option<SecretRef>,
    /// A reference only: a password in configuration is a password in every
    /// backup of it.
    pub password: Option<SecretRef>,
    /// In place of `password`, for an RDS user that authenticates with IAM.
    pub iam: Option<IamSection>,
    /// Absent says nothing was written; the default is applied where a server
    /// is built. Keeping it an `Option` is what lets a SQLite section be told
    /// that `tls` is a setting of another engine.
    pub tls: Option<DbTls>,
    pub ca_file: Option<String>,
    pub connect_timeout_secs: Option<u64>,
    /// Only a database that says so accepts `execute`.
    #[serde(default)]
    pub allow_write: bool,
    pub tunnel: Option<TunnelSection>,
    #[serde(flatten)]
    pub limits: DbLimits,
}

/// One database with the settings of its engine and nothing else.
#[derive(Clone, Debug)]
pub enum DbConnection {
    Sqlite(SqliteDatabase),
    Server(Box<ServerDatabase>),
}

#[derive(Clone, Debug)]
pub struct SqliteDatabase {
    pub path: PathBuf,
    pub allow_write: bool,
    pub limits: DbLimits,
}

#[derive(Clone, Debug)]
pub struct ServerDatabase {
    pub engine: DbEngine,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: SecretRef,
    pub auth: DbAuth,
    pub tls: DbTls,
    pub ca_file: Option<PathBuf>,
    pub connect_timeout_secs: u64,
    pub allow_write: bool,
    pub tunnel: Option<Tunnel>,
    pub limits: DbLimits,
    /// Whether the host names an Aurora DSQL cluster: PostgreSQL's protocol
    /// without `statement_timeout`, `pg_backend_pid` and `pg_cancel_backend`,
    /// and its own IAM token. Decided once, when the section is read.
    pub aurora_dsql: bool,
}

/// How long a connection may take to open when the section names nothing.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

impl DbConnection {
    pub fn engine(&self) -> DbEngine {
        match self {
            Self::Sqlite(_) => DbEngine::Sqlite,
            Self::Server(database) => database.engine,
        }
    }

    pub fn limits(&self) -> &DbLimits {
        match self {
            Self::Sqlite(database) => &database.limits,
            Self::Server(database) => &database.limits,
        }
    }

    pub fn allow_write(&self) -> bool {
        match self {
            Self::Sqlite(database) => database.allow_write,
            Self::Server(database) => database.allow_write,
        }
    }

    /// What `status` and the result envelope call this database: the file for
    /// SQLite, the database name for a server.
    pub fn database(&self) -> String {
        match self {
            Self::Sqlite(database) => database.path.display().to_string(),
            Self::Server(database) => database.database.clone(),
        }
    }

    /// The server this connects to, for the envelope. A file has none.
    pub fn host(&self) -> Option<String> {
        match self {
            Self::Sqlite(_) => None,
            Self::Server(database) => Some(format!("{}:{}", database.host, database.port)),
        }
    }

    /// A SQLite file named on the command line instead of configured. It is
    /// read-only: `execute` needs a database someone wrote `allow_write` for.
    pub fn ad_hoc_sqlite(path: &str) -> Self {
        Self::Sqlite(SqliteDatabase {
            path: PathBuf::from(path),
            allow_write: false,
            limits: DbLimits::default(),
        })
    }
}

impl TunnelSection {
    fn validated(&self) -> Result<Tunnel, DbError> {
        if self.aws_profile.trim().is_empty() {
            return Err(InvalidDb::TunnelProfileRequired.into());
        }
        let instance = match (
            self.instance_name
                .as_deref()
                .filter(|v| !v.trim().is_empty()),
            self.instance_id.as_deref().filter(|v| !v.trim().is_empty()),
        ) {
            (Some(_), Some(_)) => return Err(InvalidDb::TunnelInstanceConflict.into()),
            (Some(name), None) => InstanceRef::Name(name.to_owned()),
            (None, Some(id)) => InstanceRef::Id(id.to_owned()),
            (None, None) => return Err(InvalidDb::TunnelInstanceRequired.into()),
        };
        if self
            .region
            .as_deref()
            .is_some_and(|region| region.trim().is_empty())
        {
            return Err(InvalidDb::TunnelRegionEmpty.into());
        }
        Ok(Tunnel {
            aws_profile: self.aws_profile.clone(),
            instance,
            region: self.region.clone(),
        })
    }
}

impl IamSection {
    fn validate(&self) -> Result<(), DbError> {
        if self.aws_profile.trim().is_empty() {
            return Err(InvalidDb::IamProfileRequired.into());
        }
        if self
            .region
            .as_deref()
            .is_some_and(|region| region.trim().is_empty())
        {
            return Err(InvalidDb::IamRegionEmpty.into());
        }
        Ok(())
    }
}

impl DbSection {
    pub fn connection(&self) -> Result<DbConnection, DbError> {
        self.limits.validate()?;
        match self.engine {
            DbEngine::Sqlite => self.sqlite(),
            DbEngine::Postgresql | DbEngine::Mysql => self.server(),
        }
    }

    fn sqlite(&self) -> Result<DbConnection, DbError> {
        // Destructured without `..`: a field added to a section has to be
        // named here as one engine's or the other's, instead of being quietly
        // accepted by both. A setting of another engine is never ignored, it
        // would look applied -- and `tls` was, because a non-`Option` could
        // not tell `verify-full` written out from `verify-full` by default.
        let Self {
            engine: _,
            path: _,
            allow_write: _,
            limits: _,
            host,
            port,
            database,
            username,
            password,
            iam,
            tls,
            ca_file,
            connect_timeout_secs,
            tunnel,
        } = self;
        if host.is_some()
            || port.is_some()
            || database.is_some()
            || username.is_some()
            || password.is_some()
            || iam.is_some()
            || ca_file.is_some()
            || connect_timeout_secs.is_some()
            || tunnel.is_some()
            || tls.is_some()
        {
            return Err(InvalidDb::SqliteServerSettings.into());
        }
        let path = self.path.as_deref().ok_or(InvalidDb::SqlitePathRequired)?;
        if !Path::new(path).is_absolute() {
            return Err(InvalidDb::SqlitePathMustBeAbsolute.into());
        }
        Ok(DbConnection::Sqlite(SqliteDatabase {
            path: PathBuf::from(path),
            allow_write: self.allow_write,
            limits: self.limits.clone(),
        }))
    }

    fn server(&self) -> Result<DbConnection, DbError> {
        if self.path.is_some() {
            return Err(InvalidDb::ServerPath.into());
        }
        let text = |value: &Option<String>| {
            value
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        };
        let host = text(&self.host).ok_or(InvalidDb::ServerHostRequired)?;
        let database = text(&self.database).ok_or(InvalidDb::ServerDatabaseRequired)?;
        let username = self
            .username
            .clone()
            .ok_or(InvalidDb::ServerUsernameRequired)?;
        let auth = match (&self.password, &self.iam) {
            (Some(_), Some(_)) => return Err(InvalidDb::PasswordAndIam.into()),
            (Some(password), None) if password.is_reference() => DbAuth::Password(password.clone()),
            // A literal password would live in the file and in every copy of it.
            (Some(_), None) => return Err(InvalidDb::PasswordMustBeAReference.into()),
            (None, Some(iam)) => {
                iam.validate()?;
                DbAuth::Iam(iam.clone())
            }
            (None, None) => return Err(InvalidDb::ServerPasswordRequired.into()),
        };
        let ca_file = match self.ca_file.as_deref() {
            Some(path) if !Path::new(path).is_absolute() => {
                return Err(InvalidDb::CaFileMustBeAbsolute.into());
            }
            Some(path) => Some(PathBuf::from(path)),
            None => None,
        };
        let aurora_dsql = names_aurora_dsql(&host);
        if aurora_dsql && !(self.engine == DbEngine::Postgresql && matches!(auth, DbAuth::Iam(_))) {
            return Err(InvalidDb::AuroraDsqlSettings.into());
        }
        // The token is signed for the host as a URL names it.
        if matches!(auth, DbAuth::Iam(_)) && url::Host::parse(&host).is_err() {
            return Err(InvalidDb::IamHostInvalid.into());
        }
        // A server that says nothing about TLS gets the strictest setting.
        let tls = self.tls.unwrap_or_default();
        if ca_file.is_some() && tls == DbTls::Disable {
            return Err(InvalidDb::CaFileNeedsTls.into());
        }
        if matches!(auth, DbAuth::Iam(_)) && tls == DbTls::Disable {
            return Err(InvalidDb::IamNeedsTls.into());
        }
        let tunnel = match &self.tunnel {
            Some(section) => Some(section.validated()?),
            None => None,
        };
        // A tunnel makes the TCP target a local port, and sqlx uses one name
        // for the target and for the certificate. Checking the host name of a
        // certificate against `localhost` cannot succeed.
        if tunnel.is_some() && tls == DbTls::VerifyFull {
            return Err(InvalidDb::TunnelNeedsVerifyCa.into());
        }
        let connect_timeout_secs = match self.connect_timeout_secs {
            Some(0) => return Err(InvalidDb::PositiveLimits.into()),
            Some(seconds) => seconds,
            None => DEFAULT_CONNECT_TIMEOUT_SECS,
        };
        Ok(DbConnection::Server(Box::new(ServerDatabase {
            engine: self.engine,
            host,
            port: self.port.unwrap_or_else(|| self.engine.default_port()),
            database,
            username,
            auth,
            tls,
            ca_file,
            connect_timeout_secs,
            allow_write: self.allow_write,
            tunnel,
            limits: self.limits.clone(),
            aurora_dsql,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::config::Config;

    fn connection(toml: &str) -> DbConnection {
        Config::parse(toml).unwrap().db["one"].connection().unwrap()
    }

    #[test]
    fn db_config_accepts_a_sqlite_file_and_its_limits() {
        let connection =
            connection("[db.one]\nengine='sqlite'\npath='/tmp/app.sqlite3'\nmax_rows=10\n");
        assert_eq!(connection.limits().max_rows, 10);
        assert_eq!(connection.limits().query_timeout_secs, 30);
        assert!(!connection.allow_write());
        assert_eq!(connection.database(), "/tmp/app.sqlite3");
        assert_eq!(connection.host(), None);
    }

    #[test]
    fn db_config_gives_a_server_its_default_port_and_verifies_the_host_by_default() {
        for (engine, port) in [("postgresql", 5432), ("mysql", 3306)] {
            let connection = connection(&format!(
                "[db.one]\nengine='{engine}'\nhost='db.example.com'\ndatabase='app'\n\
                 username='reader'\npassword='op://Agent/item/password'\n"
            ));
            let DbConnection::Server(server) = &connection else {
                panic!("expected a server");
            };
            assert_eq!(server.port, port);
            assert_eq!(server.tls, DbTls::VerifyFull);
            assert_eq!(server.connect_timeout_secs, 10);
            assert_eq!(
                server.username,
                SecretRef::Literal("reader".into()),
                "a username may be written out"
            );
            assert_eq!(connection.database(), "app");
            assert_eq!(
                connection.host().as_deref(),
                Some(&format!("db.example.com:{port}")[..])
            );
        }
    }

    #[test]
    fn db_config_refuses_a_section_a_connection_cannot_be_made_from() {
        // Every section names itself in the failure it causes.
        let named = Config::parse("[db.one]\nengine='sqlite'\n").unwrap_err();
        assert!(named.to_string().contains("[db.one]"), "{named}");
        const SERVER: &str = "[db.one]\nengine='postgresql'\nhost='db'\ndatabase='app'\n\
                              username='reader'\npassword='op://Agent/item/password'\n";
        for invalid in [
            // The engine must be one kurama has.
            "[db.one]\nengine='oracle'\npath='/tmp/a.db'\n".to_owned(),
            // SQLite needs an absolute file and nothing a server owns.
            "[db.one]\nengine='sqlite'\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='app.sqlite3'\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nhost='db'\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\ntls='disable'\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nconnect_timeout_secs=5\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nunknown=1\n".to_owned(),
            // A server needs a host, a database and both credentials.
            SERVER.replace("host='db'\n", ""),
            SERVER.replace("host='db'", "host='  '"),
            SERVER.replace("database='app'\n", ""),
            SERVER.replace("username='reader'\n", ""),
            SERVER.replace("password='op://Agent/item/password'\n", ""),
            // A password lives in 1Password, not in the file.
            SERVER.replace("op://Agent/item/password", "hunter2"),
            // A server has no file, and a CA file is an absolute path that TLS uses.
            format!("{SERVER}path='/tmp/a.db'\n"),
            format!("{SERVER}ca_file='ca.pem'\n"),
            format!("{SERVER}ca_file='/tmp/ca.pem'\ntls='disable'\n"),
            format!("{SERVER}connect_timeout_secs=0\n"),
            // Limits stay usable.
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nmax_rows=0\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nmax_result_bytes=1\n".to_owned(),
            "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\nquery_timeout_secs=0\n".to_owned(),
        ] {
            assert!(Config::parse(&invalid).is_err(), "{invalid} was accepted");
        }
    }

    const IAM_SERVER: &str = "[db.one]\nengine='mysql'\nhost='app.abc.ap-northeast-1.rds.amazonaws.com'\n\
                                  database='app'\nusername='iam_reader'\n\
                                  [db.one.iam]\naws_profile='dev'\n";

    #[test]
    fn db_config_takes_an_iam_section_in_place_of_a_password() {
        let DbConnection::Server(server) = connection(IAM_SERVER) else {
            panic!("expected a server");
        };
        assert_eq!(
            server.auth,
            DbAuth::Iam(IamSection {
                aws_profile: "dev".into(),
                region: None,
            })
        );
        let DbConnection::Server(server) = connection(&format!("{IAM_SERVER}region='us-west-2'\n"))
        else {
            panic!("expected a server");
        };
        let DbAuth::Iam(iam) = server.auth else {
            panic!("expected IAM");
        };
        assert_eq!(iam.region.as_deref(), Some("us-west-2"));
    }

    #[test]
    fn db_config_refuses_an_iam_section_no_token_can_be_made_from() {
        for (invalid, says) in [
            // One way to authenticate, not two that can disagree.
            (
                IAM_SERVER.replace(
                    "[db.one.iam]",
                    "password='op://Agent/item/password'\n[db.one.iam]",
                ),
                "not both",
            ),
            (
                IAM_SERVER.replace("aws_profile='dev'", "aws_profile=' '"),
                "iam needs aws_profile",
            ),
            (
                format!("{IAM_SERVER}region=''\n"),
                "an iam region must not be empty",
            ),
            (format!("{IAM_SERVER}unknown=1\n"), "unknown"),
            // The token is the password, and RDS takes it over TLS only.
            (
                IAM_SERVER.replace("[db.one.iam]", "tls='disable'\n[db.one.iam]"),
                "tls must not be \"disable\"",
            ),
            // The token is signed for the host, so it has to be a name.
            (
                IAM_SERVER.replace("app.abc.ap-northeast-1.rds.amazonaws.com", "not a host"),
                "this host is not one",
            ),
            // A file has no user to authenticate.
            (
                "[db.one]\nengine='sqlite'\npath='/tmp/a.db'\n[db.one.iam]\naws_profile='dev'\n"
                    .to_owned(),
                "engine = \"sqlite\"",
            ),
        ] {
            let error = Config::parse(&invalid).expect_err(&invalid);
            assert!(format!("{error:#}").contains(says), "{invalid}: {error:#}");
        }
    }

    /// Aurora DSQL is told from its host once, here, and everything later
    /// reads the answer: a cluster is PostgreSQL with IAM and nothing else.
    #[test]
    fn db_config_tells_aurora_dsql_from_its_host_and_refuses_what_it_cannot_be() {
        const DSQL: &str = "[db.one]\nengine='postgresql'\n\
                            host='abcdefghijklmnopqrst.dsql.us-east-1.on.aws'\n\
                            database='postgres'\nusername='admin'\n";
        const IAM: &str = "[db.one.iam]\naws_profile='dev'\n";
        let DbConnection::Server(server) = connection(&format!("{DSQL}{IAM}")) else {
            panic!("expected a server");
        };
        assert!(server.aurora_dsql);
        let DbConnection::Server(server) = connection(IAM_SERVER) else {
            panic!("expected a server");
        };
        assert!(!server.aurora_dsql, "an RDS host is not a cluster");
        for invalid in [
            format!("{}{IAM}", DSQL.replace("postgresql", "mysql")),
            format!("{DSQL}password='op://Agent/item/password'\n"),
        ] {
            let error = Config::parse(&invalid).expect_err(&invalid);
            assert!(
                format!("{error:#}").contains("an Aurora DSQL host takes"),
                "{invalid}: {error:#}"
            );
        }
    }

    /// A password lives where secrets live, and kurama reads three of those.
    /// The rule is that it is a reference, not that it is 1Password.
    #[test]
    fn db_config_takes_a_password_from_every_store_and_a_literal_from_none() {
        const SERVER: &str = "[db.one]\nengine='postgresql'\nhost='db.example.com'\n\
                              database='app'\nusername='reader'\n";
        for reference in [
            "op://Agent/item/password",
            "aws-secrets://dev/app/db-AbCdEf#password",
            "aws-ssm://dev/app/db-password?region=us-west-2",
        ] {
            let DbConnection::Server(server) =
                connection(&format!("{SERVER}password='{reference}'\n"))
            else {
                panic!("expected a server");
            };
            let DbAuth::Password(password) = &server.auth else {
                panic!("{reference} did not become a password");
            };
            assert_eq!(
                SecretRef::parse(reference).unwrap(),
                *password,
                "{reference}"
            );
        }
        // The value itself, and a scheme that is one letter off a real one.
        for (refused, says) in [
            ("hunter2", "password must be a reference"),
            ("aws-secret://dev/app/db", "unknown secret reference scheme"),
        ] {
            let error =
                Config::parse(&format!("{SERVER}password='{refused}'\n")).expect_err(refused);
            assert!(format!("{error:#}").contains(says), "{refused}: {error:#}");
        }
        // A username may still be written out, and reads from a store too.
        let DbConnection::Server(server) = connection(
            "[db.one]\nengine='postgresql'\nhost='db.example.com'\ndatabase='app'\n\
             username='aws-secrets://dev/app/db#username'\n\
             password='aws-secrets://dev/app/db#password'\n",
        ) else {
            panic!("expected a server");
        };
        assert!(server.username.is_reference());
    }

    #[test]
    fn db_config_keeps_a_password_out_of_every_debug_of_it() {
        let connection = connection(
            "[db.one]\nengine='mysql'\nhost='db'\ndatabase='app'\n\
             username='reader'\npassword='op://Agent/item/password'\n",
        );
        let printed = format!("{connection:?}");
        assert!(printed.contains("op://Agent/item/password"), "{printed}");
        let literal = SecretRef::Literal("hunter2".into());
        assert!(!format!("{literal:?}").contains("hunter2"));
        let from_aws = super::tests::connection(
            "[db.one]\nengine='mysql'\nhost='db'\ndatabase='app'\n\
             username='reader'\npassword='aws-secrets://dev/app/db#password'\n",
        );
        assert!(
            format!("{from_aws:?}").contains("aws-secrets://dev/app/db#password"),
            "{from_aws:?}"
        );
    }
}
