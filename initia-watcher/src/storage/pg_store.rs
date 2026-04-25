use sqlx::PgPool;

use crate::errors::{Result, WatcherError};

/// Checkpoint-only store — wraps the shared sqlx pool from OrderbookProvider.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates the checkpoint table if it does not already exist.
    pub async fn ensure_checkpoint_table(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS watcher_block_numbers (
                chain_name   TEXT    NOT NULL,
                htlc_address TEXT    NOT NULL,
                block_number BIGINT  NOT NULL,
                updated_at   TIMESTAMPTZ DEFAULT now(),
                PRIMARY KEY (chain_name, htlc_address)
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WatcherError::Database(e.to_string()))?;
        Ok(())
    }

    pub async fn get_checkpoint(
        &self,
        chain_name: &str,
        htlc_address: &str,
    ) -> Result<Option<u64>> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT block_number FROM watcher_block_numbers
             WHERE chain_name = $1 AND LOWER(htlc_address) = LOWER($2)",
        )
        .bind(chain_name)
        .bind(htlc_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WatcherError::Database(e.to_string()))?;

        Ok(row.map(|(n,)| n as u64))
    }

    pub async fn update_checkpoint(
        &self,
        chain_name: &str,
        htlc_address: &str,
        block: u64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO watcher_block_numbers (chain_name, htlc_address, block_number, updated_at)
             VALUES ($1, LOWER($2), $3, NOW())
             ON CONFLICT (chain_name, htlc_address)
             DO UPDATE SET block_number = EXCLUDED.block_number, updated_at = NOW()",
        )
        .bind(chain_name)
        .bind(htlc_address)
        .bind(block as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| WatcherError::Database(e.to_string()))?;
        Ok(())
    }
}
