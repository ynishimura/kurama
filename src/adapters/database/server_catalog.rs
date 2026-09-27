//! The catalog PostgreSQL and MySQL answer from: schemas, tables and columns.
//!
//! Split from `server.rs` along the one seam that is really there. The rest of
//! that file is one connection's lifecycle -- connect, guard, prepare, read,
//! execute -- where the order is the meaning and `prepare` being in one place
//! is what a real server had to teach us. These four touch none of it: they
//! send a catalog query and read rows back, and they are what grows when an
//! engine is added.
use super::rows;
use super::server::{ServerSession, Wire, driver_error, on_either_engine};
use super::{Listing, Page};
use crate::adapters::config::DbEngine;
use crate::domain::types::database::{DbError, DbResult, ServerError};
use serde_json::Value;
use sqlx::AssertSqlSafe;

impl ServerSession {
    /// The schemas a caller may read; the engine's own are left out.
    pub async fn schemas(&mut self, page: &Page) -> Result<Listing, DbError> {
        let engine = self.access.database.engine;
        let after = page
            .after
            .as_ref()
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let sql = match engine {
            DbEngine::Postgresql => format!(
                "SELECT nspname FROM pg_namespace WHERE {} \
                 AND nspname > $1 ORDER BY nspname LIMIT $2",
                postgres_own_schemas("nspname", self.access.database.aurora_dsql)
            ),
            DbEngine::Mysql | DbEngine::Sqlite => {
                "SELECT schema_name FROM information_schema.schemata \
                 WHERE schema_name NOT IN ('mysql', 'performance_schema', 'sys', 'information_schema') \
                 AND schema_name > ? ORDER BY schema_name LIMIT ?"
                    .to_owned()
            }
        };
        let names: Vec<String> = self
            .fetch_scalars(&sql, &after, page.limit.saturating_add(1) as i64)
            .await?;
        Ok(rows::paged(
            names,
            page.limit,
            &["schema"],
            |name| vec![Value::String(name.clone())],
            |name| (name.clone(), name.clone()),
        ))
    }

    /// The tables and views of one schema, or of every readable schema.
    pub async fn tables(&mut self, schema: Option<&str>, page: &Page) -> Result<Listing, DbError> {
        let engine = self.access.database.engine;
        let (after_schema, after_name) = match &page.after {
            Some((schema, name)) => (schema.clone(), name.clone()),
            None => (String::new(), String::new()),
        };
        let sql = match engine {
            DbEngine::Postgresql => format!(
                "SELECT table_schema, table_name, table_type FROM information_schema.tables \
                 WHERE {} \
                 AND ($1 = '' OR table_schema = $1) \
                 AND (table_schema, table_name) > ($2, $3) \
                 ORDER BY table_schema, table_name LIMIT $4",
                postgres_own_schemas("table_schema", self.access.database.aurora_dsql)
            ),
            DbEngine::Mysql | DbEngine::Sqlite => {
                "SELECT table_schema, table_name, table_type FROM information_schema.tables \
                 WHERE table_schema NOT IN ('mysql', 'performance_schema', 'sys', 'information_schema') \
                 AND (? = '' OR table_schema = ?) \
                 AND (table_schema, table_name) > (?, ?) \
                 ORDER BY table_schema, table_name LIMIT ?"
                    .to_owned()
            }
        };
        let wanted = schema.unwrap_or_default().to_owned();
        let limit = page.limit.saturating_add(1) as i64;
        let engine_name = engine;
        let found: Vec<(String, String, String)> = match &mut self.wire {
            Wire::Postgres(connection) => sqlx::query_as(AssertSqlSafe(sql.clone()))
                .bind(wanted)
                .bind(after_schema)
                .bind(after_name)
                .bind(limit)
                .fetch_all(&mut **connection)
                .await
                .map_err(|error| driver_error(engine_name, error))?,
            Wire::MySql(connection) => sqlx::query_as(AssertSqlSafe(sql.clone()))
                .bind(&wanted)
                .bind(&wanted)
                .bind(after_schema)
                .bind(after_name)
                .bind(limit)
                .fetch_all(&mut **connection)
                .await
                .map_err(|error| driver_error(engine_name, error))?,
        };
        Ok(rows::paged(
            found,
            page.limit,
            &["schema", "name", "type"],
            |row| {
                vec![
                    Value::String(row.0.clone()),
                    Value::String(row.1.clone()),
                    Value::String(row.2.clone()),
                ]
            },
            |row| (row.0.clone(), row.1.clone()),
        ))
    }

    /// One table's columns, with the key and the reference each one carries.
    pub async fn describe(
        &mut self,
        schema: Option<&str>,
        table: &str,
    ) -> Result<DbResult, DbError> {
        let engine = self.access.database.engine;
        let schema = schema.unwrap_or_default().to_owned();
        // A catalog answer kurama reads itself is read completely.
        let sql = match engine {
            DbEngine::Postgresql => {
                "SELECT c.column_name, c.data_type, c.is_nullable, \
                        (c.column_default IS NOT NULL)::text, \
                        COALESCE(k.constraint_type = 'PRIMARY KEY', false)::text, \
                        COALESCE(f.referenced, '') \
                 FROM information_schema.columns c \
                 LEFT JOIN LATERAL ( \
                   SELECT tc.constraint_type FROM information_schema.key_column_usage u \
                   JOIN information_schema.table_constraints tc \
                     ON tc.constraint_name = u.constraint_name \
                    AND tc.table_schema = u.table_schema \
                   WHERE u.table_schema = c.table_schema AND u.table_name = c.table_name \
                     AND u.column_name = c.column_name AND tc.constraint_type = 'PRIMARY KEY' \
                   LIMIT 1) k ON true \
                 LEFT JOIN LATERAL ( \
                   SELECT ccu.table_name || '.' || ccu.column_name AS referenced \
                   FROM information_schema.key_column_usage u \
                   JOIN information_schema.table_constraints tc \
                     ON tc.constraint_name = u.constraint_name \
                    AND tc.table_schema = u.table_schema \
                   JOIN information_schema.constraint_column_usage ccu \
                     ON ccu.constraint_name = u.constraint_name \
                   WHERE u.table_schema = c.table_schema AND u.table_name = c.table_name \
                     AND u.column_name = c.column_name AND tc.constraint_type = 'FOREIGN KEY' \
                   LIMIT 1) f ON true \
                 WHERE ($1 = '' OR c.table_schema = $1) AND c.table_name = $2 \
                 ORDER BY c.ordinal_position"
            }
            DbEngine::Mysql | DbEngine::Sqlite => {
                "SELECT c.column_name, c.data_type, c.is_nullable, \
                        IF(c.column_default IS NULL, 'false', 'true'), \
                        IF(c.column_key = 'PRI', 'true', 'false'), \
                        COALESCE(CONCAT(k.referenced_table_name, '.', k.referenced_column_name), '') \
                 FROM information_schema.columns c \
                 LEFT JOIN information_schema.key_column_usage k \
                   ON k.table_schema = c.table_schema AND k.table_name = c.table_name \
                  AND k.column_name = c.column_name AND k.referenced_table_name IS NOT NULL \
                 WHERE (? = '' OR c.table_schema = ?) AND c.table_name = ? \
                 ORDER BY c.ordinal_position"
            }
        };
        let found: Vec<(String, String, String, String, String, String)> = match &mut self.wire {
            Wire::Postgres(connection) => sqlx::query_as(AssertSqlSafe(sql.to_owned()))
                .bind(&schema)
                .bind(table)
                .fetch_all(&mut **connection)
                .await
                .map_err(|error| driver_error(engine, error))?,
            Wire::MySql(connection) => sqlx::query_as(AssertSqlSafe(sql.to_owned()))
                .bind(&schema)
                .bind(&schema)
                .bind(table)
                .fetch_all(&mut **connection)
                .await
                .map_err(|error| driver_error(engine, error))?,
        };
        if found.is_empty() {
            return Err(DbError::Rejected(ServerError::new(
                match engine {
                    DbEngine::Postgresql => "42P01",
                    DbEngine::Mysql | DbEngine::Sqlite => "42S02",
                },
                &format!("no such table: {table}"),
            )));
        }
        let rows = found
            .into_iter()
            .map(|(name, data_type, nullable, default, key, reference)| {
                vec![
                    Value::String(name),
                    Value::String(data_type),
                    Value::String((nullable == "YES").to_string()),
                    Value::String(default),
                    Value::String(key),
                    if reference.is_empty() {
                        Value::Null
                    } else {
                        Value::String(reference)
                    },
                ]
            })
            .collect();
        Ok(rows::text_result(&super::DESCRIBE_COLUMNS, rows))
    }

    async fn fetch_scalars(
        &mut self,
        sql: &str,
        after: &str,
        limit: i64,
    ) -> Result<Vec<String>, DbError> {
        let engine = self.access.database.engine;
        on_either_engine!(&mut self.wire, |connection| sqlx::query_scalar(
            AssertSqlSafe(sql.to_owned())
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&mut **connection)
        .await
        .map_err(|error| driver_error(engine, error)))
    }
}

/// The condition on `column` that leaves out the schemas the server keeps for
/// itself. Aurora DSQL adds `sys` (`sys.jobs`, `sys.iam_pg_role_mappings`); on
/// PostgreSQL a schema named `sys` is a user's.
fn postgres_own_schemas(column: &str, aurora_dsql: bool) -> String {
    let own = if aurora_dsql {
        "('information_schema', 'sys')"
    } else {
        "('information_schema')"
    };
    format!("{column} NOT LIKE 'pg\\_%' AND {column} NOT IN {own}")
}

#[cfg(test)]
mod tests {
    use super::postgres_own_schemas;

    #[test]
    fn aurora_dsql_keeps_its_sys_schema_out_of_the_catalog() {
        assert_eq!(
            postgres_own_schemas("nspname", true),
            "nspname NOT LIKE 'pg\\_%' AND nspname NOT IN ('information_schema', 'sys')"
        );
    }

    #[test]
    fn postgresql_lists_a_schema_named_sys() {
        assert_eq!(
            postgres_own_schemas("table_schema", false),
            "table_schema NOT LIKE 'pg\\_%' AND table_schema NOT IN ('information_schema')"
        );
    }
}
