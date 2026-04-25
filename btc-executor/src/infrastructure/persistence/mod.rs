use crate::errors::PersistenceError;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub mod bitcoin_wallet;

pub use bitcoin_wallet::PgBitcoinWalletStore;

pub async fn connect_pool(database_url: &str, schema: &str, max_connections: u32) -> sqlx::Result<PgPool> {
    let schema = sanitize_schema_name(schema);
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                let create_schema = format!("CREATE SCHEMA IF NOT EXISTS {schema}");
                sqlx::query(&create_schema).execute(&mut *conn).await?;

                let set_search_path = format!("SET search_path TO {schema}, public");
                sqlx::query(&set_search_path).execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await?;

    Ok(pool)
}

pub fn database_schema(chain_identifier: &str) -> String {
    const BASE: &str = "btc_executor";

    let suffix = chain_identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if suffix.is_empty() {
        BASE.to_string()
    } else {
        format!("{BASE}_{suffix}")
    }
}

pub(super) fn map_sqlx_error(err: sqlx::Error) -> PersistenceError {
    if let sqlx::Error::Database(db_err) = &err {
        match db_err.code().as_deref() {
            Some("23505") => return PersistenceError::Conflict(db_err.message().to_string()),
            Some("23503") => return PersistenceError::NotFound(db_err.message().to_string()),
            _ => {},
        }
    }
    PersistenceError::Query(err.to_string())
}

fn sanitize_schema_name(schema: &str) -> String {
    let normalized = schema
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("_");

    if normalized.is_empty() {
        "btc_executor".to_string()
    } else if normalized
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
    {
        format!("btc_executor_{normalized}")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Row;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers_modules::postgres::Postgres;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;
    use uuid::Uuid;

    use super::connect_pool;

    #[tokio::test]
    async fn migrates_in_isolated_schema_when_public_migration_history_is_stale() {
        let local_admin_url = "postgres://postgres:postgres@localhost:5432/postgres".to_string();
        let (admin_url, _container) = if PgPoolOptions::new()
            .max_connections(1)
            .connect(&local_admin_url)
            .await
            .is_ok()
        {
            (local_admin_url, None)
        } else {
            let container = Postgres::default()
                .start()
                .await
                .expect("start postgres container");
            let host = container.get_host().await.expect("postgres host");
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("postgres port");
            (
                format!("postgres://postgres:postgres@{host}:{port}/postgres"),
                Some(container),
            )
        };
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("connect postgres admin pool");
        let db_name = format!("btc_executor_{}", Uuid::new_v4().simple());
        let create = format!("CREATE DATABASE \"{db_name}\"");
        sqlx::query(&create)
            .execute(&admin_pool)
            .await
            .expect("create test database");

        let mut db_url = reqwest::Url::parse(&admin_url).expect("valid admin url");
        db_url.set_path(&format!("/{db_name}"));
        let db_url = db_url.to_string();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&db_url)
            .await
            .expect("connect contaminated database");

        sqlx::query(
            "CREATE TABLE public._sqlx_migrations (
                version BIGINT PRIMARY KEY,
                description TEXT NOT NULL,
                installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
                success BOOLEAN NOT NULL,
                checksum BYTEA NOT NULL,
                execution_time BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create stale public migration table");
        sqlx::query(
            "INSERT INTO public._sqlx_migrations (
                version, description, success, checksum, execution_time
             ) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(20260101000000_i64)
        .bind("schema")
        .bind(true)
        .bind(vec![0_u8])
        .bind(0_i64)
        .execute(&pool)
        .await
        .expect("insert stale public migration row");

        let isolated_pool = connect_pool(&db_url, "btc_executor_test", 5)
            .await
            .expect("connect isolated pool");
        sqlx::migrate!("./migrations")
            .run(&isolated_pool)
            .await
            .expect("run isolated migrations");

        let public_versions = sqlx::query("SELECT version FROM public._sqlx_migrations")
            .fetch_all(&isolated_pool)
            .await
            .expect("load public migration versions")
            .into_iter()
            .map(|row| row.get::<i64, _>("version"))
            .collect::<Vec<_>>();
        assert_eq!(public_versions, vec![20260101000000_i64]);

        let isolated_versions =
            sqlx::query("SELECT version FROM btc_executor_test._sqlx_migrations")
                .fetch_all(&isolated_pool)
                .await
                .expect("load isolated migration versions")
                .into_iter()
                .map(|row| row.get::<i64, _>("version"))
                .collect::<Vec<_>>();
        assert_eq!(isolated_versions, vec![20260324000000_i64]);

        let current_schema = sqlx::query("SELECT current_schema() AS current_schema")
            .fetch_one(&isolated_pool)
            .await
            .expect("read current schema")
            .get::<String, _>("current_schema");
        assert_eq!(current_schema, "btc_executor_test");

        let relation_name = sqlx::query(
            "SELECT to_regclass('btc_executor_test.bitcoin_wallet_requests')::text AS relation_name",
        )
        .fetch_one(&isolated_pool)
        .await
        .expect("lookup isolated wallet table")
        .get::<Option<String>, _>("relation_name");
        assert_eq!(relation_name.as_deref(), Some("bitcoin_wallet_requests"));

        drop(isolated_pool);
        drop(pool);
        let drop_database = format!("DROP DATABASE \"{db_name}\"");
        sqlx::query(&drop_database)
            .execute(&admin_pool)
            .await
            .expect("drop test database");
    }

    #[test]
    fn derives_schema_name_from_chain_identifier() {
        assert_eq!(
            super::database_schema("bitcoin-testnet"),
            "btc_executor_bitcoin_testnet"
        );
        assert_eq!(super::database_schema(""), "btc_executor");
    }
}
