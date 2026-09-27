//! How one connection to a PostgreSQL or MySQL server opens: where it goes,
//! the password or IAM token it authenticates with, TLS, and what a refusal
//! to connect is reported as.

use super::server::Wire;
use crate::adapters::aws::db_iam_token::TokenSigner;
use crate::adapters::config::{DbAuth, DbEngine, DbTls, ServerDatabase};
use crate::domain::types::database::{DbError, InvalidDb, ServerError};
use sqlx::ConnectOptions;
use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use std::time::Duration;

/// What a connection authenticates with. Neither derives `Debug`.
#[derive(Clone)]
pub enum ServerPassword {
    /// Read from 1Password before the connection was opened.
    Fixed(String),
    /// Signed when a connection opens, because a token lives fifteen minutes
    /// and the connection that stops a statement may open later than that.
    Iam(TokenSigner),
}

/// Everything one connection needs, including the secrets it was given. It
/// derives no `Debug`: the password is in it.
#[derive(Clone)]
pub struct ServerAccess {
    pub database: ServerDatabase,
    /// Where the TCP connection really goes. A tunnel makes this a local port
    /// while `database.host` stays the name the certificate carries.
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: ServerPassword,
}

impl ServerAccess {
    /// The password this connection opens with.
    fn password(&self) -> Result<String, DbError> {
        match &self.password {
            ServerPassword::Fixed(password) => Ok(password.clone()),
            ServerPassword::Iam(signer) => signer
                .token(chrono::Utc::now())
                .map_err(|detail| InvalidDb::IamTokenUnsignable(detail.into()).into()),
        }
    }

    fn postgres(&self) -> Result<PgConnectOptions, DbError> {
        let database = &self.database;
        let mut options = PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&database.database)
            .username(&self.username)
            .password(&self.password()?)
            .ssl_mode(match database.tls {
                DbTls::VerifyFull => PgSslMode::VerifyFull,
                DbTls::VerifyCa => PgSslMode::VerifyCa,
                DbTls::Disable => PgSslMode::Disable,
            })
            .disable_statement_logging();
        if let Some(ca) = &database.ca_file {
            options = options.ssl_root_cert(ca);
        }
        Ok(options)
    }

    fn mysql(&self) -> Result<MySqlConnectOptions, DbError> {
        let database = &self.database;
        let mut options = MySqlConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&database.database)
            .username(&self.username)
            .password(&self.password()?)
            .ssl_mode(match database.tls {
                DbTls::VerifyFull => MySqlSslMode::VerifyIdentity,
                DbTls::VerifyCa => MySqlSslMode::VerifyCa,
                DbTls::Disable => MySqlSslMode::Disabled,
            })
            .enable_cleartext_plugin(takes_cleartext_token(&database.auth))
            .disable_statement_logging();
        if let Some(ca) = &database.ca_file {
            options = options.ssl_ca(ca);
        }
        Ok(options)
    }

    pub(super) async fn connect(&self) -> Result<Wire, DbError> {
        let deadline = Duration::from_secs(self.database.connect_timeout_secs);
        let opened = match self.database.engine {
            DbEngine::Postgresql => {
                let options = self.postgres()?;
                tokio::time::timeout(deadline, options.connect())
                    .await
                    .map(|result| result.map(|c| Wire::Postgres(Box::new(c))))
            }
            DbEngine::Mysql => {
                let options = self.mysql()?;
                tokio::time::timeout(deadline, options.connect())
                    .await
                    .map(|result| result.map(|c| Wire::MySql(Box::new(c))))
            }
            DbEngine::Sqlite => return Err(InvalidDb::SqliteServerSettings.into()),
        };
        match opened {
            Ok(Ok(wire)) => Ok(wire),
            Ok(Err(error)) => Err(connect_error(self.database.engine, error)),
            Err(_) => Err(DbError::unreachable(format!(
                "no answer from {}:{} within {}s",
                self.host, self.port, self.database.connect_timeout_secs
            ))),
        }
    }
}

/// MySQL takes an IAM token through `mysql_clear_password` and nothing else.
/// The configuration refused IAM without TLS, so the token never crosses the
/// network in the clear; a password never needs the plugin, and a server that
/// asks for it is not given one.
pub(super) fn takes_cleartext_token(auth: &DbAuth) -> bool {
    match auth {
        DbAuth::Iam(_) => true,
        DbAuth::Password(_) => false,
    }
}

/// A failure to open the connection. Nothing was sent.
fn connect_error(engine: DbEngine, error: sqlx::Error) -> DbError {
    // The server answered and refused: credentials, database, or a
    // requirement the client did not meet.
    if let sqlx::Error::Database(database) = &error {
        return DbError::Rejected(ServerError::new(
            database
                .code()
                .map(|code| code.into_owned())
                .unwrap_or_default(),
            database.message(),
        ));
    }
    // `Detail` is what cuts the driver's text to one bounded line.
    DbError::unreachable(format!("{} ({error})", engine.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::SecretRef;
    use crate::domain::types::database::DbLimits;

    fn access(engine: DbEngine) -> ServerAccess {
        ServerAccess {
            database: ServerDatabase {
                engine,
                host: "db.example.com".into(),
                port: 5432,
                database: "app".into(),
                username: SecretRef::Literal("reader".into()),
                auth: DbAuth::Password(SecretRef::Literal("unused".into())),
                tls: DbTls::VerifyCa,
                ca_file: None,
                connect_timeout_secs: 1,
                allow_write: false,
                tunnel: None,
                limits: DbLimits::default(),
                aurora_dsql: false,
            },
            host: "127.0.0.1".into(),
            port: 15432,
            username: "reader".into(),
            password: ServerPassword::Fixed("s3cret".into()),
        }
    }

    /// The options go where the connection really goes -- a tunnel's local
    /// port -- with the database, the user, the password and the TLS mode
    /// the section names.
    #[test]
    fn db_server_connects_where_the_access_points_with_its_credentials() {
        let access = access(DbEngine::Mysql);
        assert_eq!(access.password().unwrap(), "s3cret");
        let mysql = access.mysql().unwrap();
        assert_eq!(mysql.get_host(), "127.0.0.1");
        assert_eq!(mysql.get_port(), 15432);
        assert_eq!(mysql.get_database(), Some("app"));
        assert_eq!(mysql.get_username(), "reader");
        assert!(matches!(mysql.get_ssl_mode(), MySqlSslMode::VerifyCa));
        let postgres = super::ServerAccess {
            database: ServerDatabase {
                engine: DbEngine::Postgresql,
                ..access.database.clone()
            },
            ..access
        }
        .postgres()
        .unwrap();
        assert_eq!(postgres.get_host(), "127.0.0.1");
        assert_eq!(postgres.get_port(), 15432);
        assert_eq!(postgres.get_database(), Some("app"));
        assert_eq!(postgres.get_username(), "reader");
        assert!(matches!(postgres.get_ssl_mode(), PgSslMode::VerifyCa));
    }
}
