//! Open a database the way both `kurama db` and its explorer do -- the
//! configured section or a SQLite file, every secret read and the tunnel up
//! before a socket opens -- and wait for one statement under its deadline and
//! a stop, naming why it ended.
use crate::adapters::aws::db_iam_token::TokenSigner;
use crate::adapters::aws::ssm_tunnel::{self, OpenTunnel, TunnelRequest};
use crate::adapters::config::{Config, DbAuth, DbConnection, IamSection, ServerDatabase};
use crate::adapters::database::server::{ServerAccess, ServerPassword};
use crate::adapters::database::{Cancellation, DbTarget};
use crate::adapters::secret_resolver::ConfiguredSecrets;
use crate::domain::functions::signing_target::infer_from_host;
use crate::domain::types::Profile;
use crate::domain::types::database::{DbError, DbFailure, InvalidDb, StatementEnd};
use crate::ports::{AwsProfileCredentials, SecretResolver};
use crate::shell::aws_profile_credentials::AssumedRoles;
use crate::shell::cli::commands::db_command::is_sqlite_file;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

/// How one statement ended, and whether the wait for it was given up.
pub struct Waited<T> {
    pub outcome: Result<T, DbError>,
    /// The statement nobody could stop still holds the connection, so it must
    /// be dropped rather than closed or used again.
    pub abandoned: bool,
}

/// Wait for `work` until it ends, its deadline passes or `interrupt` fires.
///
/// Dropping the future does not stop the engine, so a deadline or an
/// interrupt sets the flag it reads and then waits for the statement to end;
/// a read nobody could stop is given up instead (see [`abandons_the_wait`]).
pub async fn wait_for_statement<T>(
    work: impl Future<Output = Result<T, DbError>>,
    cancel: &Cancellation,
    timeout_secs: u64,
    interrupt: impl Future<Output = ()>,
    writes: bool,
) -> Waited<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut ended: Option<DbFailure> = None;
    let mut abandoned = false;
    let outcome = {
        let mut work = std::pin::pin!(work);
        let mut interrupt = std::pin::pin!(interrupt);
        loop {
            tokio::select! {
                result = &mut work => break result,
                _ = tokio::time::sleep_until(deadline), if ended.is_none() => {
                    let stopped = cancel.stop().await;
                    let end = StatementEnd::after_a_cancel(stopped);
                    ended = Some(DbFailure::Timeout { seconds: timeout_secs, end });
                    if abandons_the_wait(stopped, writes) {
                        abandoned = true;
                        break Err(DbFailure::Interrupted { end }.into());
                    }
                }
                _ = &mut interrupt, if ended.is_none() => {
                    let stopped = cancel.stop().await;
                    let end = StatementEnd::after_a_cancel(stopped);
                    ended = Some(DbFailure::Interrupted { end });
                    if abandons_the_wait(stopped, writes) {
                        abandoned = true;
                        break Err(DbFailure::Interrupted { end }.into());
                    }
                }
            }
        }
    };
    Waited {
        outcome: name_why_it_ended(outcome, ended, writes),
        abandoned,
    }
}

/// Whether to stop waiting for a statement nobody could stop. A read leaves
/// nothing behind, so waiting for it is waiting for nothing: Aurora DSQL has
/// no cancel at all, and its own transaction limit is the only other end. A
/// write is waited for, because what it kept is the answer the caller needs,
/// and the session sends no COMMIT once a stop was asked.
fn abandons_the_wait(stopped: bool, writes: bool) -> bool {
    !stopped && !writes
}

/// The engine reports every stop the same way; only the waiting loop knows
/// why it was asked. How the statement ended is the session's answer and not
/// the loop's guess, so the two are merged by [`how_it_really_ended`].
fn name_why_it_ended<T>(
    outcome: Result<T, DbError>,
    ended: Option<DbFailure>,
    writes: bool,
) -> Result<T, DbError> {
    match (outcome, ended) {
        (Err(DbError::Failed(DbFailure::Interrupted { end: session })), Some(reason)) => {
            Err(DbError::Failed(match reason {
                DbFailure::Timeout { seconds, end } => DbFailure::Timeout {
                    seconds,
                    end: how_it_really_ended(end, session),
                },
                DbFailure::Interrupted { end } => DbFailure::Interrupted {
                    end: how_it_really_ended(end, session),
                },
                other => other,
            }))
        }
        // A write the session let through was committed before anyone
        // asked: the session sends no COMMIT once a stop was asked.
        (Ok(done), Some(_)) if writes => Ok(done),
        // A read that answers after its stop was asked cannot be told from a
        // complete one: MySQL's `SLEEP()` and `BENCHMARK()` answer a
        // `KILL QUERY` with a row, as if they had finished.
        (Ok(_), Some(reason)) => Err(reason.into()),
        // The database said something else first, which is the better answer.
        (Err(error), Some(_)) => Err(error),
        (outcome, None) => outcome,
    }
}

/// How the statement ended, between what the cancel reported and what the
/// session did afterwards. The session is the one that watched the statement
/// to its end, so its answer wins wherever it has one.
fn how_it_really_ended(asked: StatementEnd, session: StatementEnd) -> StatementEnd {
    match session {
        // The statement ran to its end and the change was rolled back before
        // the next statement or the COMMIT: nothing was kept, and no cancel
        // that was sent makes that a stop the database made.
        StatementEnd::RolledBack => StatementEnd::RolledBack,
        // The engine itself reported the stop, whatever the cancel got back.
        StatementEnd::Stopped => StatementEnd::Stopped,
        // The session has nothing to add, so what the cancel reported stands.
        StatementEnd::NotConfirmed => asked,
    }
}

/// The database to open, with the secrets a server needs already read and the
/// bastion in place when it needs one. A file needs neither, so nothing
/// consults 1Password or AWS for it.
pub async fn open_target(
    connection: &DbConnection,
    config: &Config,
) -> anyhow::Result<(DbTarget, Option<OpenTunnel>)> {
    let database = match connection {
        DbConnection::Sqlite(database) => {
            return Ok((DbTarget::Sqlite(database.clone()), None));
        }
        DbConnection::Server(database) => database,
    };
    // One cache in front of the shared AssumeRole path: the tunnel, the IAM
    // token and every `aws-*://` secret reference of this call go through it,
    // so one AWS profile is assumed once however many of them name it.
    let roles: Arc<AssumedRoles> = Arc::new(AssumedRoles::of_config(Arc::new(config.clone())));
    // What needs no credentials, before anything that does: a token nobody
    // can sign is not worth a TOTP, and not worth the tunnel's either.
    let auth = match &database.auth {
        DbAuth::Iam(iam) => {
            let profile = roles.load_profile(&iam.aws_profile).await?;
            let region = iam_region(iam, &database.host, profile.region_raw())?;
            PlannedAuth::Iam { profile, region }
        }
        DbAuth::Password(reference) => PlannedAuth::Password(reference),
    };
    // The tunnel next: a database nothing can reach is not worth asking
    // 1Password or AWS for its password, and a person answering a prompt for a
    // call that cannot work is a prompt nobody should have seen.
    let tunnel = open_tunnel(database, roles.as_ref()).await?;
    let secrets = ConfiguredSecrets::new(
        &config.onepassword,
        Arc::clone(&roles) as Arc<dyn AwsProfileCredentials>,
    );
    let username = match secrets.resolve(&database.username).await {
        // A user name is who connects, not a credential: the IAM token is
        // signed for it and a refusal names it.
        Ok(username) => username.expose().to_owned(),
        Err(error) => return close_and_fail(tunnel, error.into()).await,
    };
    // A token is signed for the database's own host and port even behind a
    // tunnel, because that is the name the database checks; AWS is asked for
    // the role and for nothing else, once when the tunnel took the same one.
    let password = match auth {
        PlannedAuth::Iam { profile, region } => {
            roles.assume_role(&profile).await.map(|credentials| {
                ServerPassword::Iam(TokenSigner {
                    host: database.host.clone(),
                    port: database.port,
                    username: username.clone(),
                    region,
                    aurora_dsql: database.aurora_dsql,
                    credentials,
                })
            })
        }
        PlannedAuth::Password(reference) => secrets
            .resolve(reference)
            .await
            .map(ServerPassword::Fixed)
            .map_err(Into::into),
    };
    let password = match password {
        Ok(password) => password,
        Err(error) => return close_and_fail(tunnel, error).await,
    };
    // With a tunnel the TCP target is the local port the plugin took; the
    // configured host stays what the certificate is checked against, which is
    // why a tunnel cannot verify the host name.
    let (host, port) = match &tunnel {
        Some(tunnel) => ("localhost".to_owned(), tunnel.local_port),
        None => (database.host.clone(), database.port),
    };
    Ok((
        DbTarget::Server(Box::new(ServerAccess {
            host,
            port,
            database: (**database).clone(),
            username,
            password,
        })),
        tunnel,
    ))
}

/// How the user will authenticate, with everything decided that needs no
/// credentials.
enum PlannedAuth<'a> {
    Password(&'a crate::domain::types::SecretRef),
    Iam { profile: Profile, region: String },
}

/// A tunnel that opened and then had nothing to carry is still closed.
async fn close_and_fail<T>(tunnel: Option<OpenTunnel>, error: anyhow::Error) -> anyhow::Result<T> {
    if let Some(tunnel) = tunnel {
        tunnel.close().await;
    }
    Err(error)
}

/// The bastion, with the role credentials of the AWS profile it names.
async fn open_tunnel(
    database: &ServerDatabase,
    roles: &AssumedRoles,
) -> anyhow::Result<Option<OpenTunnel>> {
    let Some(tunnel) = &database.tunnel else {
        return Ok(None);
    };
    // Before AWS is asked for anything: a plugin that would take the session
    // token on its command line cannot be used, whatever the credentials say.
    let ready = ssm_tunnel::require_usable_plugin().await?;
    let profile = roles.load_profile(&tunnel.aws_profile).await?;
    let region = tunnel
        .region
        .clone()
        .or_else(|| profile.region_raw().map(str::to_owned))
        .ok_or(DbError::from(InvalidDb::TunnelRegionEmpty))?;
    let credentials = roles.assume_role(&profile).await?;
    Ok(Some(
        OpenTunnel::open(
            TunnelRequest {
                credentials: &credentials,
                region,
                instance: tunnel.instance.clone(),
                remote_host: database.host.clone(),
                remote_port: database.port,
                connect_timeout_secs: database.connect_timeout_secs,
            },
            ready,
        )
        .await?,
    ))
}

/// Where the token is signed for: what the section says, then the region an
/// RDS host name carries, then the AWS profile's.
fn iam_region(
    iam: &IamSection,
    host: &str,
    profile_region: Option<&str>,
) -> Result<String, DbError> {
    iam.region
        .clone()
        .or_else(|| infer_from_host(host).region)
        .or_else(|| profile_region.map(str::to_owned))
        .ok_or_else(|| InvalidDb::IamRegionUnknown.into())
}

/// The `[db.*]` section, or a SQLite file the command line named directly.
pub fn resolve(name: &str, config: &Config) -> Result<DbConnection, DbError> {
    if is_sqlite_file(name) {
        return Ok(DbConnection::ad_hoc_sqlite(name));
    }
    config
        .db_connection(name)
        .cloned()
        .ok_or_else(|| InvalidDb::UnknownDatabase.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read nobody could stop is not waited for; a write is, because what
    /// it kept is the answer, and an engine that confirmed the stop is about
    /// to end the statement itself.
    #[test]
    fn db_only_a_read_nobody_could_stop_is_abandoned() {
        assert!(abandons_the_wait(false, false));
        assert!(!abandons_the_wait(false, true));
        assert!(!abandons_the_wait(true, false));
        assert!(!abandons_the_wait(true, true));
    }

    #[test]
    fn db_a_stop_is_named_by_why_it_was_asked_and_whether_it_was_confirmed() {
        let interrupted = |end| Err::<(), _>(DbError::Failed(DbFailure::Interrupted { end }));
        let timeout = |end| Some(DbFailure::Timeout { seconds: 7, end });
        // The loop could not confirm it and the engine did: it stopped.
        assert!(matches!(
            name_why_it_ended(
                interrupted(StatementEnd::Stopped),
                timeout(StatementEnd::NotConfirmed),
                false
            ),
            Err(DbError::Failed(DbFailure::Timeout {
                seconds: 7,
                end: StatementEnd::Stopped
            }))
        ));
        // Nobody confirmed it, which is what Aurora DSQL always answers.
        assert!(matches!(
            name_why_it_ended(
                interrupted(StatementEnd::NotConfirmed),
                timeout(StatementEnd::NotConfirmed),
                false
            ),
            Err(DbError::Failed(DbFailure::Timeout {
                end: StatementEnd::NotConfirmed,
                ..
            }))
        ));
        assert!(matches!(
            name_why_it_ended(
                interrupted(StatementEnd::NotConfirmed),
                Some(DbFailure::Interrupted {
                    end: StatementEnd::Stopped
                }),
                false
            ),
            Err(DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::Stopped
            }))
        ));
        // Another failure stays what it is.
        assert!(matches!(
            name_why_it_ended(
                Err::<(), _>(DbFailure::Disconnected.into()),
                timeout(StatementEnd::Stopped),
                false
            ),
            Err(DbError::Failed(DbFailure::Disconnected))
        ));
        assert!(matches!(
            name_why_it_ended(interrupted(StatementEnd::Stopped), None, false),
            Err(DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::Stopped
            }))
        ));
    }

    /// MySQL's `SLEEP()` and `BENCHMARK()` answer a `KILL QUERY` with a row,
    /// as if they had finished: a read that answers after its stop was asked
    /// cannot be told from a complete one, so it fails the way the stop was
    /// asked. A change that answers was committed before anyone asked -- the
    /// session sends no COMMIT once a stop was asked -- so it stands.
    #[test]
    fn db_a_read_answered_after_its_stop_was_asked_is_not_a_result() {
        let timeout = || {
            Some(DbFailure::Timeout {
                seconds: 1,
                end: StatementEnd::Stopped,
            })
        };
        assert!(matches!(
            name_why_it_ended(Ok(()), timeout(), false),
            Err(DbError::Failed(DbFailure::Timeout {
                seconds: 1,
                end: StatementEnd::Stopped
            }))
        ));
        assert!(matches!(
            name_why_it_ended(
                Ok(()),
                Some(DbFailure::Interrupted {
                    end: StatementEnd::NotConfirmed
                }),
                false
            ),
            Err(DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::NotConfirmed
            }))
        ));
        assert!(name_why_it_ended(Ok(()), timeout(), true).is_ok());
        assert!(name_why_it_ended(Ok(()), None, false).is_ok());
    }

    /// The engine nobody could tell to stop, whose statement ran to its end,
    /// and whose change the session rolled back before the COMMIT: what
    /// happened to the change is what the message says, never a stop nobody
    /// made. A real Aurora DSQL cluster answered exactly this on 2026-09-21,
    /// where a 2-second timeout on an 8-second statement read "the database
    /// confirmed it stopped" and DSQL had stopped nothing.
    #[test]
    fn db_a_change_rolled_back_after_a_stop_nobody_confirmed_says_nothing_was_kept() {
        let rolled_back = || {
            Err::<(), _>(DbError::Failed(DbFailure::Interrupted {
                end: StatementEnd::RolledBack,
            }))
        };
        let error = name_why_it_ended(
            rolled_back(),
            Some(DbFailure::Timeout {
                seconds: 2,
                end: StatementEnd::NotConfirmed,
            }),
            true,
        )
        .expect_err("the change failed");
        assert_eq!(
            error.to_string(),
            "the statement timed out after 2 seconds and the change was rolled back, \
             so nothing was kept; narrow it or raise --timeout"
        );
        let error = name_why_it_ended(
            rolled_back(),
            Some(DbFailure::Interrupted {
                end: StatementEnd::NotConfirmed,
            }),
            true,
        )
        .expect_err("the change failed");
        assert_eq!(
            error.to_string(),
            "the statement was interrupted and the change was rolled back, so nothing was kept"
        );
        // A cancel the engine did answer does not make it a stop either: the
        // statement ended by itself and the change is what was undone.
        let error = name_why_it_ended(
            rolled_back(),
            Some(DbFailure::Timeout {
                seconds: 2,
                end: StatementEnd::Stopped,
            }),
            true,
        )
        .expect_err("the change failed");
        assert!(
            error.to_string().contains("the change was rolled back"),
            "{error}"
        );
    }

    #[test]
    fn db_iam_region_is_the_sections_then_the_hosts_then_the_profiles() {
        const RDS: &str = "app.abc.ap-northeast-1.rds.amazonaws.com";
        let iam = |region: Option<&str>| IamSection {
            aws_profile: "dev".into(),
            region: region.map(str::to_owned),
        };
        let region = |iam: &IamSection, host, profile| iam_region(iam, host, profile).ok();
        assert_eq!(
            region(&iam(Some("us-west-2")), RDS, Some("eu-west-1")).as_deref(),
            Some("us-west-2")
        );
        assert_eq!(
            region(&iam(None), RDS, Some("eu-west-1")).as_deref(),
            Some("ap-northeast-1")
        );
        // An alias says nothing about where the database is.
        assert_eq!(
            region(&iam(None), "db.internal.example.com", Some("eu-west-1")).as_deref(),
            Some("eu-west-1")
        );
        assert!(matches!(
            iam_region(&iam(None), "db.internal.example.com", None),
            Err(DbError::Invalid(InvalidDb::IamRegionUnknown))
        ));
    }
}
