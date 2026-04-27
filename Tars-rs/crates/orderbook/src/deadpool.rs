use deadpool::managed::{Manager, Metrics, RecycleResult};
use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, Connection, Error as SqlxError, PgConnection};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DbPool {
    options: PgConnectOptions,
}

impl DbPool {
    pub fn new(db_url: &str, max_size: usize) -> eyre::Result<Pool> {
        let options: PgConnectOptions = db_url.parse()?;

        Ok(Pool::builder(Self { options })
            .max_size(max_size)
            .wait_timeout(Some(Duration::from_secs(5)))
            .create_timeout(Some(Duration::from_secs(10)))
            .recycle_timeout(Some(Duration::from_secs(10)))
            .runtime(deadpool::Runtime::Tokio1)
            .build()?)
    }
}

impl Manager for DbPool {
    type Type = PgConnection;
    type Error = SqlxError;

    async fn create(&self) -> Result<PgConnection, SqlxError> {
        self.options.connect().await
    }

    async fn recycle(&self, obj: &mut Self::Type, _: &Metrics) -> RecycleResult<SqlxError> {
        Ok(obj.ping().await?)
    }
}

pub type Pool = deadpool::managed::Pool<DbPool>;
