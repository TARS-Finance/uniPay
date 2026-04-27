//! Low level DB calls to get order details
use super::primitives;
use crate::{
    errors::OrderbookError,
    primitives::{
        Claim, MatchedOrderVerbose, OrderQueryFilters, OrderStatusVerbose, PaginatedData,
        SingleSwap, StatsQueryFilters, SwapChain,
    },
    traits::Orderbook,
};
use async_trait::async_trait;
use bigdecimal::num_bigint::{BigInt, ToBigInt};
use bigdecimal::{BigDecimal, FromPrimitive};
use chrono::{DateTime, Utc};
use eyre::Result;
use serde_json::json;
use sqlx::{Pool, Postgres, QueryBuilder, Row, Transaction};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
// The `Orderbook` trait implementation. Provides various orderbook queries direct from db.
pub struct OrderbookProvider {
    pub pool: Pool<Postgres>,
}

impl OrderbookProvider {
    pub fn new(pool: Pool<Postgres>) -> Self {
        OrderbookProvider { pool }
    }

    pub async fn from_db_url(db_url: &str) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2000)
            .connect(db_url)
            .await?;
        Ok(Self::new(pool))
    }
}

#[async_trait]
impl Orderbook for OrderbookProvider {
    /// Returns swap associated with given order_id and chain
    async fn get_swap(
        &self,
        order_id: &str,
        chain: SwapChain,
    ) -> Result<Option<primitives::SingleSwap>, OrderbookError> {
        let statement = match chain {
            SwapChain::Source =>  "SELECT * FROM swaps JOIN matched_orders mo on swaps.swap_id = mo.source_swap_id WHERE mo.create_order_id = $1",
            SwapChain::Destination => "SELECT * FROM swaps JOIN matched_orders mo on swaps.swap_id = mo.destination_swap_id WHERE mo.create_order_id = $1",
        };

        let swap = sqlx::query_as::<_, SingleSwap>(&statement)
            .bind(order_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(swap)
    }

    async fn get_solver_committed_funds(
        &self,
        addr: &str,
        chain: &str,
        asset: &str,
    ) -> Result<BigDecimal, OrderbookError> {
        // lowercase the address
        let addr = addr.to_lowercase();
        let asset = asset.to_lowercase();
        let current_time = Utc::now().timestamp();
        let destination_swap_stm = "SELECT coalesce(SUM(s2.amount),0) as total_locked_amount FROM matched_orders mo
                           JOIN create_orders co ON mo.create_order_id = co.create_id
                           JOIN swaps s1 ON mo.source_swap_id = s1.swap_id
                           JOIN swaps s2 ON mo.destination_swap_id = s2.swap_id
                           WHERE (co.additional_data->>'deadline')::integer > $1 AND s2.chain = $2 AND s2.initiate_block_number = 0 AND s1.initiate_tx_hash != '' AND lower(s2.initiator) = $3 AND lower(s2.asset) = $4";

        let amount = sqlx::query_scalar::<_, BigDecimal>(destination_swap_stm)
            .bind(current_time)
            .bind(chain)
            .bind(&addr)
            .bind(&asset)
            .fetch_optional(&self.pool)
            .await?
            .unwrap_or(BigDecimal::from(0));

        Ok(amount)
    }

    async fn exists(&self, secret_hash: &str) -> Result<bool, OrderbookError> {
        const EXISTS_QUERY: &str =
            "SELECT EXISTS(SELECT 1 FROM create_orders WHERE secret_hash = $1)";
        let exists = sqlx::query_scalar::<_, bool>(EXISTS_QUERY)
            .bind(secret_hash)
            .fetch_one(&self.pool)
            .await?;

        Ok(exists)
    }

    async fn get_all_matched_orders(
        &self,
        filters: OrderQueryFilters,
    ) -> Result<PaginatedData<primitives::MatchedOrderVerbose>, OrderbookError> {
        const BASE_JOINS: &str = "FROM matched_orders mo
            JOIN create_orders co ON mo.create_order_id = co.create_id
            JOIN swaps ss1 ON mo.source_swap_id = ss1.swap_id
            JOIN swaps ss2 ON mo.destination_swap_id = ss2.swap_id";

        let orders = {
            let mut builder = QueryBuilder::<Postgres>::new(
                "SELECT
                    mo.created_at,
                    mo.updated_at,
                    mo.deleted_at,
                    co.*,
                    row_to_json(ss1.*) as source_swap,
                    row_to_json(ss2.*) as destination_swap
                    ",
            );
            builder.push(BASE_JOINS);

            if let Some(address) = &filters.address {
                builder.add_address_filter(address);
            } else {
                if let Some(from) = &filters.from_owner {
                    builder.add_from_owner_filter(from);
                }

                if let Some(to) = &filters.to_owner {
                    builder.add_to_owner_filter(to);
                }
            }

            if let Some(tx_hash) = &filters.tx_hash {
                builder.add_tx_hash_filter(tx_hash);
            }

            if let Some(from_chain) = &filters.from_chain {
                builder.add_chain_filter("ss1.chain", from_chain.as_ref());
            }

            if let Some(to_chain) = &filters.to_chain {
                builder.add_chain_filter("ss2.chain", to_chain.as_ref());
            }

            if let Some(statuses) = &filters.status {
                builder.add_statuses_filter(&statuses);
            }

            builder.push(" ORDER BY mo.created_at DESC LIMIT ");
            builder.push_bind(filters.per_page());
            builder.push(" OFFSET ");
            builder.push_bind(filters.offset());

            builder
                .build_query_as::<primitives::MatchedOrderVerbose>()
                .fetch_all(&self.pool)
                .await?
        };

        // Build count query
        let item_count = {
            let mut builder = QueryBuilder::<Postgres>::new("SELECT COUNT(*) ");
            builder.push(BASE_JOINS);

            if let Some(address) = &filters.address {
                builder.add_address_filter(address);
            } else {
                if let Some(from) = &filters.from_owner {
                    builder.add_from_owner_filter(from);
                }

                if let Some(to) = &filters.to_owner {
                    builder.add_to_owner_filter(to);
                }
            }

            if let Some(tx_hash) = &filters.tx_hash {
                builder.add_tx_hash_filter(tx_hash);
            }

            if let Some(from_chain) = &filters.from_chain {
                builder.add_chain_filter("ss1.chain", from_chain.as_ref());
            }

            if let Some(to_chain) = &filters.to_chain {
                builder.add_chain_filter("ss2.chain", to_chain.as_ref());
            }

            if let Some(statuses) = &filters.status {
                builder.add_statuses_filter(statuses);
            }

            builder
                .build_query_scalar::<i64>()
                .fetch_one(&self.pool)
                .await?
        };

        // Return paginated data
        Ok(PaginatedData::new(
            orders,
            filters.page(),
            item_count,
            filters.per_page(),
        ))
    }

    async fn get_matched_order(
        &self,
        create_id: &str,
    ) -> Result<Option<primitives::MatchedOrderVerbose>, OrderbookError> {
        // Query for retrieving a specific matched order with its related data
        const MATCHED_ORDER_QUERY: &str = "SELECT
        mo.created_at,
        mo.updated_at,
        mo.deleted_at,
        co.*,
        row_to_json(ss1.*) as source_swap,
        row_to_json(ss2.*) as destination_swap
        FROM matched_orders mo
        JOIN create_orders co ON mo.create_order_id = co.create_id
        JOIN swaps ss1 ON mo.source_swap_id = ss1.swap_id
        JOIN swaps ss2 ON mo.destination_swap_id = ss2.swap_id
        WHERE mo.create_order_id = $1";

        // Fetch the matched order
        let matched_order =
            sqlx::query_as::<_, primitives::MatchedOrderVerbose>(MATCHED_ORDER_QUERY)
                .bind(create_id)
                .fetch_optional(&self.pool)
                .await?;

        Ok(matched_order)
    }

    async fn get_matched_orders(
        &self,
        user: &str,
        filters: OrderQueryFilters,
    ) -> Result<PaginatedData<primitives::MatchedOrderVerbose>, OrderbookError> {
        // Convert user address to lowercase for case-insensitive comparison
        let user_lowercase = user.to_lowercase();

        // Get current timestamp for deadline checking
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| OrderbookError::InternalError("Failed to get current time".to_string()))?
            .as_secs() as i64;

        // Define pending condition queries with clear, descriptive names
        const INIT_PENDING_CONDITION: &str =
            "ss1.initiate_tx_hash = '' AND (co.additional_data->>'deadline')::bigint > $4";
        const REDEEM_PENDING_CONDITION: &str =
            "(ss1.initiate_tx_hash != '' AND ss2.refund_tx_hash = '' AND ss2.redeem_block_number = 0 AND ss2.initiate_tx_hash != '' AND ss1.refund_block_number = 0)";
        const REFUND_PENDING_CONDITION: &str =
            "(ss1.initiate_tx_hash != '' AND ss1.redeem_tx_hash = '' AND ss1.refund_block_number = 0 AND ss2.redeem_tx_hash = '')";

        // Build base query
        let mut orders_query = String::from(
            "SELECT
            mo.created_at,
            mo.updated_at,
            mo.deleted_at,
            co.*,
            row_to_json(ss1.*) as source_swap,
            row_to_json(ss2.*) as destination_swap
            FROM matched_orders mo
            JOIN create_orders co ON mo.create_order_id = co.create_id
            JOIN swaps ss1 ON mo.source_swap_id = ss1.swap_id
            JOIN swaps ss2 ON mo.destination_swap_id = ss2.swap_id
            WHERE (lower(co.initiator_source_address) = $1 OR lower(co.user_id) = $1)",
        );

        // Add pending filter conditions if requested
        if let Some(statuses) = &filters.status {
            for status in statuses.iter() {
                match status {
                    OrderStatusVerbose::InProgress => {
                        orders_query.push_str(
                &format!(" AND ({INIT_PENDING_CONDITION} OR {REDEEM_PENDING_CONDITION} OR {REFUND_PENDING_CONDITION})")
                );
                    }
                    OrderStatusVerbose::Completed => {
                        orders_query.push_str(
                            " AND (ss2.redeem_block_number > 0 OR ss1.refund_block_number > 0)",
                        );
                    }
                    _ => {}
                }
            }
        }

        // Add ordering and pagination
        orders_query.push_str(" ORDER BY mo.created_at DESC LIMIT $2 OFFSET $3");

        // Fetch the orders with pagination
        let matched_orders = sqlx::query_as::<_, primitives::MatchedOrderVerbose>(&orders_query)
            .bind(&user_lowercase)
            .bind(filters.per_page())
            .bind(filters.offset())
            .bind(current_time)
            .fetch_all(&self.pool)
            .await?;

        // Build the count query, similar to orders query but for counting
        let mut count_query = String::from(
            "SELECT COUNT(*)
            FROM matched_orders mo
            JOIN create_orders co ON mo.create_order_id = co.create_id
            JOIN swaps ss1 ON mo.source_swap_id = ss1.swap_id
            JOIN swaps ss2 ON mo.destination_swap_id = ss2.swap_id
            WHERE (lower(co.initiator_source_address) = $1 OR lower(co.user_id) = $1)",
        );

        // For count query, use $2 instead of $4 for the timestamp parameter
        const COUNT_INIT_PENDING_CONDITION: &str =
            "ss1.initiate_tx_hash = '' AND (co.additional_data->>'deadline')::bigint > $2";

        // Add pending filter conditions to count query if requested
        if let Some(statuses) = &filters.status {
            for status in statuses.iter() {
                match status {
                    OrderStatusVerbose::InProgress => {
                        count_query.push_str(
                &format!(" AND ({COUNT_INIT_PENDING_CONDITION} OR {REDEEM_PENDING_CONDITION} OR {REFUND_PENDING_CONDITION})")
                );
                    }
                    OrderStatusVerbose::Completed => {
                        count_query.push_str(
                            " AND (ss2.redeem_block_number > 0 OR ss1.refund_block_number > 0)",
                        );
                    }
                    _ => {}
                }
            }
        }

        // Get the total count for pagination
        let item_count = sqlx::query_scalar::<_, i64>(&count_query)
            .bind(&user_lowercase)
            .bind(current_time)
            .fetch_one(&self.pool)
            .await?;

        // Return paginated data
        Ok(PaginatedData::new(
            matched_orders,
            filters.page(),
            item_count,
            filters.per_page(),
        ))
    }

    async fn add_instant_refund_sacp(
        &self,
        order_id: &str,
        instant_refund_tx_bytes: &str,
    ) -> Result<(), OrderbookError> {
        // Create the JSON payload to update order's additional_data
        let additional_data = json!({
            "instant_refund_tx_bytes": instant_refund_tx_bytes
        });

        // SQL query to update the order using PostgreSQL's JSONB concatenation operator
        const UPDATE_QUERY: &str = "UPDATE create_orders
            SET additional_data = additional_data || $2::jsonb
            WHERE create_id = $1";

        // Execute the query with parameters
        let update_result = sqlx::query(UPDATE_QUERY)
            .bind(order_id)
            .bind(additional_data)
            .execute(&self.pool)
            .await?;

        if update_result.rows_affected() == 0 {
            return Err(OrderbookError::OrderNotFound {
                order_id: order_id.to_string(),
            });
        }

        Ok(())
    }

    async fn add_redeem_sacp(
        &self,
        order_id: &str,
        redeem_tx_bytes: &str,
        redeem_tx_id: &str,
        secret: &str,
    ) -> Result<(), OrderbookError> {
        // Create the JSON payload with the redeem transaction bytes
        let additional_data = json!({
            "redeem_tx_bytes": redeem_tx_bytes
        });

        // First, update the create_orders table with the redeem transaction bytes
        const CREATE_ORDER_UPDATE: &str = "
            UPDATE create_orders
            SET additional_data = additional_data || $2::jsonb
            WHERE create_id = $1";

        let order_update_result = sqlx::query(CREATE_ORDER_UPDATE)
            .bind(order_id)
            .bind(&additional_data)
            .execute(&self.pool)
            .await?;

        if order_update_result.rows_affected() == 0 {
            return Err(OrderbookError::OrderNotFound {
                order_id: order_id.to_string(),
            });
        }

        // Then, update the corresponding destination swap with the redeem transaction hash and secret
        const DESTINATION_SWAP_UPDATE: &str = "
            UPDATE swaps
            SET redeem_tx_hash = $1, secret = $2
            WHERE swap_id = (
                SELECT destination_swap_id
                FROM matched_orders
                WHERE create_order_id = $3
            )";

        let destination_swap_update_result = sqlx::query(DESTINATION_SWAP_UPDATE)
            .bind(redeem_tx_id)
            .bind(secret)
            .bind(order_id)
            .execute(&self.pool)
            .await?;

        if destination_swap_update_result.rows_affected() == 0 {
            return Err(OrderbookError::SwapNotFound {
                order_id: order_id.to_string(),
            });
        }

        Ok(())
    }

    async fn get_filler_pending_orders(
        &self,
        chain_id: &str,
        filler_id: &str,
    ) -> Result<Vec<MatchedOrderVerbose>, OrderbookError> {
        let orders = sqlx::query_as::<_, MatchedOrderVerbose>(
            "SELECT
                mo.created_at,
                mo.updated_at,
                mo.deleted_at,
                row_to_json(ss.*) as source_swap,
                row_to_json(ds.*) as destination_swap,
                co.*
            FROM matched_orders mo
            JOIN create_orders co ON mo.create_order_id = co.create_id
            JOIN swaps ss ON mo.source_swap_id = ss.swap_id
            JOIN swaps ds ON mo.destination_swap_id = ds.swap_id
            WHERE ((ds.chain = $1) OR (ss.chain = $1))
            AND (lower(ss.redeemer) = $2 OR lower(ds.initiator) = $2)
		    AND
		    (
		    	(ss.initiate_tx_hash != '' AND ss.refund_tx_hash = '' AND ds.initiate_tx_hash = '')
		    	OR
		    	(ds.secret != '' AND ss.redeem_tx_hash = '')
		    	OR
		    	(ds.initiate_tx_hash != '' AND ds.refund_tx_hash = '' AND ds.redeem_tx_hash = '')
		    	OR
		    	(ss.refund_tx_hash = '' AND ss.redeem_tx_hash = '' AND ds.refund_block_number > 0)
		    )
            ORDER BY mo.created_at ASC
            LIMIT 1000",
        )
        .bind(chain_id)
        .bind(filler_id.to_lowercase())
        .fetch_all(&self.pool)
        .await?;
        Ok(orders)
    }

    async fn get_solver_pending_orders(&self) -> Result<Vec<MatchedOrderVerbose>, OrderbookError> {
        let orders = sqlx::query_as::<_, MatchedOrderVerbose>(
            "SELECT
                mo.created_at,
                mo.updated_at,
                mo.deleted_at,
                row_to_json(ss.*) as source_swap,
                row_to_json(ds.*) as destination_swap,
                co.*
            FROM matched_orders mo
            JOIN create_orders co ON mo.create_order_id = co.create_id
            JOIN swaps ss ON mo.source_swap_id = ss.swap_id
            JOIN swaps ds ON mo.destination_swap_id = ds.swap_id
            WHERE (
                (ss.initiate_tx_hash != '' AND ss.refund_tx_hash = '' AND ds.initiate_tx_hash = '')
                OR
                (ds.secret != '' AND ss.redeem_tx_hash = '' AND ss.refund_tx_hash = '')
                OR
                (ds.initiate_tx_hash != '' AND ds.refund_tx_hash = '' AND ds.redeem_tx_hash = '')
                OR
                (ss.refund_tx_hash = '' AND ss.redeem_tx_hash = '' AND ds.refund_block_number > 0)
            )
            ORDER BY mo.created_at ASC
            LIMIT 5000",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(orders)
    }

    async fn update_swap_initiate(
        &self,
        order_id: &str,
        filled_amount: BigDecimal,
        initiate_tx_hash: &str,
        initiate_block_number: i64,
        initiate_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError> {
        // SQL query to update swap initiation details
        const UPDATE_SWAP_QUERY: &str = "UPDATE swaps
            SET filled_amount = $1,
                initiate_tx_hash = $2,
                initiate_block_number = $3,
                initiate_timestamp = $4
            WHERE swap_id = $5";

        // Execute the query and get the result for checking rows affected
        let update_result = sqlx::query(UPDATE_SWAP_QUERY)
            .bind(&filled_amount)
            .bind(initiate_tx_hash)
            .bind(initiate_block_number)
            .bind(initiate_timestamp)
            .bind(order_id)
            .execute(&self.pool)
            .await?;

        // Check if any rows were affected by the update
        if update_result.rows_affected() == 0 {
            return Err(OrderbookError::SwapNotFound {
                order_id: order_id.to_string(),
            });
        }

        Ok(())
    }

    async fn update_swap_redeem(
        &self,
        order_id: &str,
        redeem_tx_hash: &str,
        secret: &str,
        redeem_block_number: i64,
        redeem_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError> {
        const UPDATE_SWAP_QUERY: &str = "UPDATE swaps
            SET redeem_tx_hash = $1,
                secret = $2,
                redeem_block_number = $3,
                redeem_timestamp = $4
            WHERE swap_id = $5";

        let res = sqlx::query(UPDATE_SWAP_QUERY)
            .bind(redeem_tx_hash)
            .bind(secret)
            .bind(redeem_block_number)
            .bind(redeem_timestamp)
            .bind(&order_id)
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(OrderbookError::SwapNotFound {
                order_id: order_id.to_string(),
            });
        }
        Ok(())
    }

    async fn update_swap_refund(
        &self,
        order_id: &str,
        refund_tx_hash: &str,
        refund_block_number: i64,
        refund_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError> {
        const UPDATE_SWAP_QUERY: &str = "UPDATE swaps
            SET refund_tx_hash = $1,
                refund_block_number = $2,
                refund_timestamp = $3
            WHERE swap_id = $4";

        let res = sqlx::query(UPDATE_SWAP_QUERY)
            .bind(refund_tx_hash)
            .bind(refund_block_number)
            .bind(refund_timestamp)
            .bind(&order_id)
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(OrderbookError::SwapNotFound {
                order_id: order_id.to_string(),
            });
        }
        Ok(())
    }

    async fn update_confirmations(
        &self,
        chain_identifier: &str,
        latest_block: u64,
    ) -> Result<(), OrderbookError> {
        const UPDATE_SWAP_QUERY: &str =
            "UPDATE swaps
            SET current_confirmations = LEAST(required_confirmations, $1 - initiate_block_number + 1)
            WHERE chain = $2
            AND required_confirmations > current_confirmations
            AND initiate_tx_hash != ''";

        sqlx::query(UPDATE_SWAP_QUERY)
            .bind(latest_block as i64)
            .bind(chain_identifier)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_volume_and_fees(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>,
    ) -> Result<(BigInt, BigInt), OrderbookError> {
        let start_time = match query.from {
            Some(from) => Some(DateTime::from_timestamp(from, 0).ok_or_else(|| {
                OrderbookError::InvalidTimestamp("Failed to parse from timestamp".to_string())
            })?),
            None => None,
        };

        // End time is optional, if not provided, use current time
        let end_time = match query.to {
            Some(to) => DateTime::from_timestamp(to, 0).ok_or_else(|| {
                OrderbookError::InvalidTimestamp("Failed to parse to timestamp".to_string())
            })?,
            None => Utc::now(),
        };

        const BASE_QUERY: &str = "SELECT
                    ss1.chain as source_chain,
                    ss1.asset as source_asset,
                    ss1.amount as source_amount,
                    ss2.chain as destination_chain, 
                    ss2.asset as destination_asset, 
                    ss2.amount as destination_amount,
                    (co.additional_data->>'input_token_price')::float as source_token_price,
                    (co.additional_data->>'output_token_price')::float as destination_token_price
                FROM matched_orders mo
                JOIN create_orders co ON mo.create_order_id = co.create_id
                JOIN swaps ss1 ON (ss1.swap_id = mo.source_swap_id)
                JOIN swaps ss2 ON (ss2.swap_id = mo.destination_swap_id)
                WHERE ss1.redeem_tx_hash != '' 
                AND ss2.redeem_tx_hash != ''";

        let rows = {
            let mut builder = QueryBuilder::<Postgres>::new(BASE_QUERY);

            builder.add_time_range_filter(start_time, end_time);

            if let Some(ref source_chain) = query.source_chain {
                builder.add_chain_filter("ss1.chain", &source_chain);
            }

            if let Some(ref destination_chain) = query.destination_chain {
                builder.add_chain_filter("ss2.chain", &destination_chain);
            }

            if let Some(ref address) = query.address {
                builder.add_address_filter(&address);
            }

            builder.build().fetch_all(&self.pool).await?
        };

        let mut total_volume = BigDecimal::from(0);
        let mut total_fees = BigDecimal::from(0);

        for row in rows {
            let source_chain: String = row.get("source_chain");
            let source_asset: String = row.get("source_asset");
            let source_amount: BigDecimal = row.get("source_amount");

            let destination_chain: String = row.get("destination_chain");
            let destination_asset: String = row.get("destination_asset");
            let destination_amount: BigDecimal = row.get("destination_amount");

            let source_token_price: f64 = row.get("source_token_price");
            let destination_token_price: f64 = row.get("destination_token_price");

            let source_asset_decimals =
                match asset_decimals.get(&(source_chain.clone(), source_asset.to_lowercase())) {
                    Some(d) => *d,
                    None => 8,
                };

            let destination_asset_decimals = match asset_decimals
                .get(&(destination_chain.clone(), destination_asset.to_lowercase()))
            {
                Some(d) => *d,
                None => 8,
            };

            let source_divisor = BigDecimal::from(10_u64.pow(source_asset_decimals));
            let destination_divisor = BigDecimal::from(10_u64.pow(destination_asset_decimals));

            let normalized_source_amount = &source_amount / &source_divisor;
            let normalized_destination_amount = &destination_amount / &destination_divisor;

            let source_value = normalized_source_amount
                * BigDecimal::from_f64(source_token_price).ok_or_else(|| {
                    OrderbookError::InternalError(format!(
                        "Failed to parse token price: {}",
                        source_token_price
                    ))
                })?;

            let dest_value = normalized_destination_amount
                * BigDecimal::from_f64(destination_token_price).ok_or_else(|| {
                    OrderbookError::InternalError(format!(
                        "Failed to parse token price: {}",
                        destination_token_price
                    ))
                })?;

            let fee = &source_value - &dest_value;
            total_fees += fee;

            if query.source_chain.is_some() {
                total_volume += &source_value;
            }

            if query.destination_chain.is_some() {
                total_volume += &dest_value;
            }

            if query.source_chain.is_none() && query.destination_chain.is_none() {
                total_volume += &source_value + &dest_value;
            }
        }

        let total_volume_int = total_volume.to_bigint().ok_or_else(|| {
            OrderbookError::InternalError("Failed to convert total volume to bigint".to_string())
        })?;

        let total_fees_int = total_fees.to_bigint().ok_or_else(|| {
            OrderbookError::InternalError("Failed to convert total fees to bigint".to_string())
        })?;

        Ok((total_volume_int, total_fees_int))
    }

    async fn get_volume(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>,
    ) -> Result<BigDecimal, OrderbookError> {
        let (total_volume, _) = self.get_volume_and_fees(query, asset_decimals).await?;

        Ok(BigDecimal::from(total_volume))
    }

    async fn get_fees(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>,
    ) -> Result<BigDecimal, OrderbookError> {
        let (_, total_fees) = self.get_volume_and_fees(query, asset_decimals).await?;

        Ok(BigDecimal::from(total_fees))
    }

    async fn get_integrator_fees(&self, integrator: &str) -> Result<Vec<Claim>, OrderbookError> {
        const QUERY: &str = "
            SELECT 
                integrator_name,
                address,
                chain,
                token_address,
                total_earnings,
                claim_signature,
                claim_contract
            FROM affiliate_fees 
            WHERE integrator_name = $1
        ";

        let claims = sqlx::query_as::<_, Claim>(QUERY)
            .bind(integrator)
            .fetch_all(&self.pool)
            .await?;

        Ok(claims)
    }

    async fn create_matched_order(
        &self,
        order: &MatchedOrderVerbose,
    ) -> Result<(), OrderbookError> {
        let mut tx: Transaction<'_, Postgres> = self.pool.begin().await?;

        let is_source_case_sensitive = order.source_swap.chain.contains("bitcoin")
            || order.source_swap.chain.contains("solana");
        let is_destination_case_sensitive = order.destination_swap.chain.contains("bitcoin")
            || order.destination_swap.chain.contains("solana");
        // Helper to normalize strings based on chain (Bitcoin and Solana addresses are case-sensitive)
        let normalize_address = |addr: &str, is_case_sensitive: bool| -> String {
            if is_case_sensitive {
                addr.to_string()
            } else {
                addr.to_lowercase()
            }
        };

        // Inserting both swaps in a single query
        const INSERT_SWAPS_QUERY: &str = "
            INSERT INTO swaps (
                created_at, updated_at, deleted_at, swap_id, chain, asset, initiator, redeemer, timelock, filled_amount, amount, secret_hash, secret, initiate_tx_hash, redeem_tx_hash, refund_tx_hash, initiate_block_number, redeem_block_number, refund_block_number, required_confirmations, current_confirmations, htlc_address, token_address
            ) VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23),
                ($24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41, $42, $43, $44, $45, $46)
            ON CONFLICT DO NOTHING
        ";

        let res = sqlx::query(INSERT_SWAPS_QUERY)
            // Source swap
            .bind(&order.source_swap.created_at)
            .bind(&order.source_swap.updated_at)
            .bind(&order.source_swap.deleted_at)
            .bind(&order.source_swap.swap_id)
            .bind(&order.source_swap.chain)
            .bind(&order.source_swap.asset.to_lowercase())
            .bind(normalize_address(
                &order.source_swap.initiator,
                is_source_case_sensitive,
            ))
            .bind(normalize_address(
                &order.source_swap.redeemer,
                is_source_case_sensitive,
            ))
            .bind(&order.source_swap.timelock)
            .bind(&order.source_swap.filled_amount)
            .bind(&order.source_swap.amount)
            .bind(&order.source_swap.secret_hash)
            .bind(&order.source_swap.secret)
            .bind(&order.source_swap.initiate_tx_hash.to_string())
            .bind(&order.source_swap.redeem_tx_hash.to_string())
            .bind(&order.source_swap.refund_tx_hash.to_string())
            .bind(&order.source_swap.initiate_block_number)
            .bind(&order.source_swap.redeem_block_number)
            .bind(&order.source_swap.refund_block_number)
            .bind(&order.source_swap.required_confirmations)
            .bind(&order.source_swap.current_confirmations)
            .bind(normalize_address(
                order.source_swap.htlc_address.as_deref().unwrap_or(""),
                is_source_case_sensitive,
            ))
            .bind(normalize_address(
                order.source_swap.token_address.as_deref().unwrap_or(""),
                is_source_case_sensitive,
            ))
            // Destination swap
            .bind(&order.destination_swap.created_at)
            .bind(&order.destination_swap.updated_at)
            .bind(&order.destination_swap.deleted_at)
            .bind(&order.destination_swap.swap_id)
            .bind(&order.destination_swap.chain)
            .bind(&order.destination_swap.asset.to_lowercase())
            .bind(normalize_address(
                &order.destination_swap.initiator,
                is_destination_case_sensitive,
            ))
            .bind(normalize_address(
                &order.destination_swap.redeemer,
                is_destination_case_sensitive,
            ))
            .bind(&order.destination_swap.timelock)
            .bind(&order.destination_swap.filled_amount)
            .bind(&order.destination_swap.amount)
            .bind(&order.destination_swap.secret_hash)
            .bind(&order.destination_swap.secret.to_string())
            .bind(&order.destination_swap.initiate_tx_hash.to_string())
            .bind(&order.destination_swap.redeem_tx_hash.to_string())
            .bind(&order.destination_swap.refund_tx_hash.to_string())
            .bind(&order.destination_swap.initiate_block_number)
            .bind(&order.destination_swap.redeem_block_number)
            .bind(&order.destination_swap.refund_block_number)
            .bind(&order.destination_swap.required_confirmations)
            .bind(&order.destination_swap.current_confirmations)
            .bind(normalize_address(
                order.destination_swap.htlc_address.as_deref().unwrap_or(""),
                is_destination_case_sensitive,
            ))
            .bind(normalize_address(
                order
                    .destination_swap
                    .token_address
                    .as_deref()
                    .unwrap_or(""),
                is_destination_case_sensitive,
            ))
            .execute(&mut *tx)
            .await?;

        if res.rows_affected() < 2 {
            tx.rollback().await?;
            return Err(OrderbookError::OrderAlreadyExists(
                order.create_order.create_id.clone(),
            ));
        }

        const INSERT_CREATE_ORDER_QUERY: &str = "
            INSERT INTO create_orders (
                created_at, updated_at, deleted_at, create_id, block_number, source_chain, destination_chain, source_asset, destination_asset, initiator_source_address, initiator_destination_address, source_amount, destination_amount, fee, nonce, min_destination_confirmations, timelock, secret_hash, user_id, affiliate_fees, additional_data
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21
            )
            ON CONFLICT DO NOTHING
        ";

        let co = &order.create_order;
        let additional_data_json = serde_json::to_value(&co.additional_data)
            .map_err(|e| OrderbookError::InternalError(e.to_string()))?;
        let affiliate_fees_json = serde_json::to_value(&co.affiliate_fees)
            .map_err(|e| OrderbookError::InternalError(e.to_string()))?;
        let res = sqlx::query(INSERT_CREATE_ORDER_QUERY)
            .bind(&co.created_at)
            .bind(&co.updated_at)
            .bind(&co.deleted_at)
            .bind(&co.create_id)
            .bind(&co.block_number)
            .bind(&co.source_chain)
            .bind(&co.destination_chain)
            .bind(normalize_address(
                &co.source_asset,
                is_source_case_sensitive,
            ))
            .bind(normalize_address(
                &co.destination_asset,
                is_destination_case_sensitive,
            ))
            .bind(normalize_address(
                &co.initiator_source_address,
                is_source_case_sensitive,
            ))
            .bind(normalize_address(
                &co.initiator_destination_address,
                is_destination_case_sensitive,
            ))
            .bind(&co.source_amount)
            .bind(&co.destination_amount)
            .bind(&co.fee)
            .bind(&co.nonce)
            .bind(&co.min_destination_confirmations)
            .bind(&co.timelock)
            .bind(&co.secret_hash)
            .bind(&co.user_id)
            .bind(&affiliate_fees_json)
            .bind(&additional_data_json)
            .execute(&mut *tx)
            .await?;

        if res.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(OrderbookError::OrderAlreadyExists(co.create_id.clone()));
        }

        const INSERT_MATCHED_ORDER_QUERY: &str = "
            INSERT INTO matched_orders (create_order_id, source_swap_id, destination_swap_id, created_at, updated_at, deleted_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT DO NOTHING
        ";

        let res = sqlx::query(INSERT_MATCHED_ORDER_QUERY)
            .bind(&co.create_id)
            .bind(&order.source_swap.swap_id)
            .bind(&order.destination_swap.swap_id)
            .bind(&order.create_order.created_at)
            .bind(&order.create_order.updated_at)
            .bind(&order.create_order.deleted_at)
            .execute(&mut *tx)
            .await?;

        if res.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(OrderbookError::OrderAlreadyExists(co.create_id.clone()));
        }

        tx.commit().await?;
        Ok(())
    }
}

trait OrderbookQueryBuilder<'a> {
    fn add_where_clause(&mut self);
    fn add_address_filter(&mut self, address: &'a str);
    fn add_from_owner_filter(&mut self, address: &'a str);
    fn add_to_owner_filter(&mut self, address: &'a str);
    fn add_tx_hash_filter(&mut self, tx_hash: &'a str);
    fn add_time_range_filter(&mut self, start_time: Option<DateTime<Utc>>, end_time: DateTime<Utc>);
    fn add_chain_filter(&mut self, column: &'a str, chain: &'a str);
    fn add_statuses_filter(&mut self, status: &HashSet<OrderStatusVerbose>);
}

impl<'a> OrderbookQueryBuilder<'a> for QueryBuilder<'a, Postgres> {
    fn add_where_clause(&mut self) {
        if !self.sql().contains("WHERE") {
            self.push(" WHERE ");
        } else {
            self.push(" AND ");
        }
    }

    fn add_address_filter(&mut self, address: &'a str) {
        let lower_address = address.to_lowercase();
        self.add_where_clause();
        self.push("(");
        self.push("LOWER(ss1.initiator) = ");
        self.push_bind(lower_address.clone());
        self.push(" OR LOWER(ss2.initiator) = ");
        self.push_bind(lower_address.clone());
        self.push(" OR LOWER(ss1.redeemer) = ");
        self.push_bind(lower_address.clone());
        self.push(" OR LOWER(ss2.redeemer) = ");
        self.push_bind(lower_address.clone());
        self.push(" OR LOWER(co.user_id) = ");
        self.push_bind(lower_address.clone());
        self.push(" OR (co.additional_data::jsonb->>'bitcoin_optional_recipient' IS NOT NULL AND LOWER(co.additional_data::jsonb->>'bitcoin_optional_recipient') = ");
        self.push_bind(lower_address.clone());
        self.push("))");
    }

    fn add_from_owner_filter(&mut self, address: &'a str) {
        let lower_address = address.to_lowercase();
        self.add_where_clause();
        self.push("LOWER(co.initiator_source_address) = ");
        self.push_bind(lower_address);
    }

    fn add_to_owner_filter(&mut self, address: &'a str) {
        let lower_address = address.to_lowercase();
        self.add_where_clause();
        self.push("LOWER(co.initiator_destination_address) = ");
        self.push_bind(lower_address);
    }

    fn add_tx_hash_filter(&mut self, tx_hash: &'a str) {
        let pattern = format!("%{}%", tx_hash.to_lowercase());
        self.add_where_clause();
        self.push("(");
        self.push("LOWER(ss1.initiate_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(" OR LOWER(ss2.initiate_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(" OR LOWER(ss1.refund_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(" OR LOWER(ss2.refund_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(" OR LOWER(ss1.redeem_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(" OR LOWER(ss2.redeem_tx_hash) LIKE ");
        self.push_bind(pattern.clone());
        self.push(")");
    }

    fn add_time_range_filter(&mut self, from_time: Option<DateTime<Utc>>, to_time: DateTime<Utc>) {
        if let Some(from) = from_time {
            self.add_where_clause();
            self.push("co.created_at >= ");
            self.push_bind(from);
        }
        self.add_where_clause();
        self.push("co.created_at <= ");
        self.push_bind(to_time);
    }

    fn add_chain_filter(&mut self, column: &'a str, chain: &'a str) {
        self.add_where_clause();
        self.push(column);
        self.push(" = ");
        self.push_bind(chain);
    }

    fn add_statuses_filter(&mut self, statuses: &HashSet<OrderStatusVerbose>) {
        let not_initiated = "(co.additional_data->>'deadline')::bigint > EXTRACT(EPOCH FROM NOW())::bigint AND ss1.initiate_tx_hash = ''";
        let in_progress =
            "ss1.initiate_block_number > 0 AND (co.additional_data->>'deadline')::bigint > EXTRACT(EPOCH FROM ss1.initiate_timestamp)::bigint AND ss1.redeem_tx_hash = '' AND ss1.refund_tx_hash = ''";
        let completed = "ss1.redeem_tx_hash <> '' OR ss1.refund_tx_hash <> ''";
        let expired = "(co.additional_data->>'deadline')::bigint < EXTRACT(EPOCH FROM NOW())::bigint AND ss1.initiate_tx_hash = ''";
        let refunded = "ss1.refund_tx_hash <> ''";

        self.add_where_clause();
        self.push("("); // <-- open group
        for (i, status) in statuses.iter().enumerate() {
            if i > 0 {
                self.push(" OR ");
            }

            let clause = match status {
                OrderStatusVerbose::NotInitiated => not_initiated,
                OrderStatusVerbose::InProgress => in_progress,
                OrderStatusVerbose::Completed => completed,
                OrderStatusVerbose::Expired => expired,
                OrderStatusVerbose::Refunded => refunded,
            };

            self.push("(");
            self.push(clause);
            self.push(")");
        }
        self.push(")"); // <-- close group
    }
}

#[cfg(not(feature = "test-utils"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        primitives::{AdditionalData, ChainName},
        test_utils::{
            create_test_matched_order, default_matched_order, delete_all_matched_orders,
            delete_matched_order, provider, simulate_test_swap_initiate, simulate_test_swap_redeem,
            simulate_test_swap_refund, TestMatchedOrderConfig, TestTxData,
        },
    };
    use alloy::{hex, primitives::Address};
    use serial_test::serial;
    use std::str::FromStr;
    use utils::Hashable;

    #[tokio::test]
    #[serial]
    async fn test_get_solver_committed_funds() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let filler_address = format!("bcrt1q{}", hex::encode(&rand::random::<[u8; 20]>()));
        let order_config = TestMatchedOrderConfig {
            destination_chain_initiator_address: filler_address,
            ..Default::default()
        };

        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();
        let amount = provider
            .get_solver_committed_funds(
                &order.destination_swap.initiator,
                &order.destination_swap.chain,
                &order.destination_swap.asset,
            )
            .await
            .unwrap();
        assert_eq!(amount, BigDecimal::from(0));

        // Simulate swap initiate for the source swap
        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();

        let updated_amount = provider
            .get_solver_committed_funds(
                &order.destination_swap.initiator,
                &order.destination_swap.chain,
                &order.destination_swap.asset,
            )
            .await
            .unwrap();

        println!("new committed amount {:#?}", updated_amount);
        assert_eq!(
            updated_amount.cmp(&order.destination_swap.amount),
            std::cmp::Ordering::Equal
        );

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_secret_hash_exists() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let secret: String = (0..32)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();

        let secret_hash = secret.sha256().unwrap();

        let is_exists = provider.exists(&secret_hash.to_string()).await.unwrap();

        // checking for the secret hash before creating any order.
        assert!(!is_exists);

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        let ss_swap_exists = provider
            .exists(&order.source_swap.secret_hash)
            .await
            .unwrap();
        let ds_swap_exists = provider
            .exists(&order.destination_swap.secret_hash)
            .await
            .unwrap();

        // checking for the secret hash after creating an order.
        assert!(ss_swap_exists);
        assert!(ds_swap_exists);

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_all_matched_orders() {
        let provider = provider().await;

        delete_all_matched_orders(&provider.pool).await.unwrap();
        let mut created_orders = Vec::new();
        let num_orders = 5;

        for _ in 0..num_orders {
            let order_config = TestMatchedOrderConfig {
                ..Default::default()
            };
            let order = create_test_matched_order(&provider.pool, order_config)
                .await
                .unwrap();
            created_orders.push(order);
        }

        let matched_orders = provider
            .get_all_matched_orders(OrderQueryFilters::default())
            .await
            .unwrap();

        // Verify that all matched orders exist in the retrieved list
        let created_ids: Vec<_> = created_orders
            .iter()
            .map(|o| &o.create_order.create_id)
            .collect();
        let fetched_ids: Vec<_> = matched_orders
            .data
            .iter()
            .map(|o| &o.create_order.create_id)
            .collect();
        for id in &created_ids {
            assert!(
                fetched_ids.contains(id),
                "Order with ID {} was not found!",
                id
            );
        }

        for order in created_orders {
            delete_matched_order(&provider.pool, &order.create_order.create_id)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_get_all_matched_orders_with_filters() {
        let provider = provider().await;

        delete_all_matched_orders(&provider.pool).await.unwrap();
        let address = format!(
            "0x{}",
            alloy::primitives::hex::encode(alloy_primitives::Address::random())
        );

        let order_config1 = TestMatchedOrderConfig {
            initiator_source_address: address.clone(),
            source_chain: "arbitrum_sepolia".to_string(),
            ..Default::default()
        };
        let order1 = create_test_matched_order(&provider.pool, order_config1)
            .await
            .unwrap();
        let order_config2 = TestMatchedOrderConfig::default();
        let order2 = create_test_matched_order(&provider.pool, order_config2)
            .await
            .unwrap();

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order1.create_order.create_id
        );

        // Should find all orders when no chain filters are provided
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(1, 10, None, None, None, None, None, None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 2);

        // Should find orders with only source chain filter
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("arbitrum_sepolia")),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("bitcoin_regtest")),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        // Should return empty orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("starknet_sepolia")),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("arbitrum_sepolia")),
                    Some(ChainName::new("bitcoin_regtest")),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        // Should find orders with only destination chain filter
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    Some(ChainName::new("ethereum_localnet")),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 2);

        // Should find orders with both source and destination chain filters
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("arbitrum_sepolia")),
                    Some(ChainName::new("ethereum_localnet")),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    Some(ChainName::new("bitcoin_regtest")),
                    Some(ChainName::new("ethereum_localnet")),
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        delete_matched_order(&provider.pool, &order1.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&provider.pool, &order2.create_order.create_id)
            .await
            .unwrap();

        let order_config3 = TestMatchedOrderConfig::default();
        let order3 = create_test_matched_order(&provider.pool, order_config3)
            .await
            .unwrap();

        let tx_hash = "0x1234567890123456789012345678901234567890";
        let user_id = "user_id".to_string();

        simulate_test_swap_initiate(
            &provider.pool,
            &order3.source_swap.swap_id,
            Some(TestTxData {
                tx_hash: tx_hash.to_string(),
                block_number: 132,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 3,
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap();

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    Some(tx_hash.to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0]
                .source_swap
                .initiate_tx_hash
                .to_string(),
            tx_hash
        );

        delete_matched_order(&provider.pool, &order3.create_order.create_id)
            .await
            .unwrap();
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    Some("invalid_tx_hash".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        // should return empty list as no orders match the invalid tx hash
        assert_eq!(matched_orders.data.len(), 0);

        let order_config4 = TestMatchedOrderConfig {
            initiator_source_address: address.clone(),
            ..Default::default()
        };
        let order4 = create_test_matched_order(&provider.pool, order_config4)
            .await
            .unwrap();
        let tx_hash = "0xabcdef1234567890abcdef1234567890abcdef12";

        simulate_test_swap_initiate(
            &provider.pool,
            &order4.source_swap.swap_id,
            Some(TestTxData {
                tx_hash: tx_hash.to_string(),
                block_number: 133,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 3,
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap();

        // Should find no orders when address matches but tx_hash doesn't
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone()),
                    Some("invalid_tx".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        // Should find no orders when address doesn't match
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some("invalid_address".to_string()),
                    Some(tx_hash.to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        // Should find the order with both matching address and tx_hash
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone()),
                    Some(tx_hash.to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order4.create_order.create_id
        );

        // should find order with both lowercase and uppercase address
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone().to_uppercase()),
                    Some(tx_hash.to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order4.create_order.create_id
        );

        // should find order with both lowercase and uppercase tx_hash
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone()),
                    Some(tx_hash.to_uppercase()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order4.create_order.create_id
        );

        // Clean up
        delete_matched_order(&provider.pool, &order4.create_order.create_id)
            .await
            .unwrap();

        // should handle btc tx hashes
        let order_config5 = TestMatchedOrderConfig {
            destination_chain_initiator_address: address.clone(),
            user_id: user_id.clone(),
            ..Default::default()
        };
        let order5 = create_test_matched_order(&provider.pool, order_config5)
            .await
            .unwrap();
        let btc_tx_hash = "1f7b872dfa5ab3dd814c69f10b502178f94f55f1ee4125e0f202cc4229c7a27c:898618,0x1234567890123456789012345678901234567890:100";
        simulate_test_swap_initiate(
            &provider.pool,
            &order5.destination_swap.swap_id,
            Some(TestTxData {
                tx_hash: btc_tx_hash.to_string(),
                block_number: 133,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 3,
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap();

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(address.clone()),
                    Some(
                        "1f7b872dfa5ab3dd814c69f10b502178f94f55f1ee4125e0f202cc4229c7a27c"
                            .to_string(),
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order5.create_order.create_id
        );

        // Should find order with user_id
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    Some(user_id.clone()),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.user_id.clone().unwrap(),
            user_id
        );

        delete_matched_order(&provider.pool, &order5.create_order.create_id)
            .await
            .unwrap();

        // should filter orders by status filters
        let order_config6 = TestMatchedOrderConfig::default();
        let order6 = create_test_matched_order(&provider.pool, order_config6)
            .await
            .unwrap();

        //should return initiated orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::NotInitiated])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order6.create_order.create_id
        );

        let tx_hash = "0x1264567890126456789012645678901264567890";
        simulate_test_swap_initiate(
            &provider.pool,
            &order6.source_swap.swap_id,
            Some(TestTxData {
                tx_hash: tx_hash.to_string(),
                block_number: 162,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 6,
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap();

        //should return initiated orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::InProgress])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order6.create_order.create_id
        );

        //simulate redeem
        let now = chrono::Utc::now();
        provider
            .update_swap_redeem(
                &order6.source_swap.swap_id,
                "0x1234567890123456789012345678901234567890",
                &order6.source_swap.secret_hash,
                162,
                now,
            )
            .await
            .unwrap();
        //should return completed orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order6.create_order.create_id
        );

        //should return expired orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Expired])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        // create a matched_order but update additional_data to make it expired
        let order_config7 = TestMatchedOrderConfig {
            ..Default::default()
        };
        let mut order7 = create_test_matched_order(&provider.pool, order_config7)
            .await
            .unwrap();
        // delete existing matched_order
        delete_matched_order(&provider.pool, &order7.create_order.create_id)
            .await
            .unwrap();
        let deadline = order7.create_order.additional_data.deadline - (24 * 60 * 60);
        order7.create_order.additional_data = AdditionalData {
            strategy_id: order7.create_order.additional_data.strategy_id,
            bitcoin_optional_recipient: order7
                .create_order
                .additional_data
                .bitcoin_optional_recipient,
            input_token_price: order7.create_order.additional_data.input_token_price,
            output_token_price: order7.create_order.additional_data.output_token_price,
            sig: order7.create_order.additional_data.sig,
            deadline,
            instant_refund_tx_bytes: order7.create_order.additional_data.instant_refund_tx_bytes,
            redeem_tx_bytes: order7.create_order.additional_data.redeem_tx_bytes,
            tx_hash: order7.create_order.additional_data.tx_hash,
            is_blacklisted: order7.create_order.additional_data.is_blacklisted,
            integrator: order7.create_order.additional_data.integrator,
            version: order7.create_order.additional_data.version,
            bitcoin: None,
            source_delegator: None,
        };
        provider.create_matched_order(&order7).await.unwrap();
        //should return expired orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Expired])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order7.create_order.create_id
        );

        //simulate initiate
        simulate_test_swap_initiate(&provider.pool, &order7.source_swap.swap_id, None)
            .await
            .unwrap();

        //should return refunded orders
        simulate_test_swap_refund(&provider.pool, &order7.source_swap.swap_id, None)
            .await
            .unwrap();

        //should return refunded orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Refunded])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);
        assert_eq!(
            matched_orders.data[0].create_order.create_id,
            order7.create_order.create_id
        );

        // should filter orders by status filters
        let order_config8 = TestMatchedOrderConfig::default();
        let order8 = create_test_matched_order(&provider.pool, order_config8)
            .await
            .unwrap();

        //should return initiated orders
        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([
                        OrderStatusVerbose::NotInitiated,
                        OrderStatusVerbose::Refunded,
                    ])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 2);

        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.create_id == order8.create_order.create_id,)
            .any(|x| x));

        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.create_id == order7.create_order.create_id)
            .any(|x| x));

        // Clean up
        delete_matched_order(&provider.pool, &order6.create_order.create_id)
            .await
            .unwrap();

        let order_config9 = TestMatchedOrderConfig {
            initiator_source_address: address.clone(),
            initiator_destination_address: Address::ZERO.to_string(),
            ..Default::default()
        };
        let _ = create_test_matched_order(&provider.pool, order_config9)
            .await
            .unwrap();

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(address.clone()),
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.initiator_source_address == address.to_string())
            .all(|x| x));
        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.initiator_destination_address
                == Address::ZERO.to_string())
            .all(|x| x));

        let matched_orders = provider
            .get_all_matched_orders(
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(Address::ZERO.to_string()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.initiator_source_address == address.to_string())
            .all(|x| x));
        assert!(matched_orders
            .data
            .iter()
            .map(|order| order.create_order.initiator_destination_address
                == Address::ZERO.to_string())
            .all(|x| x));
    }

    #[tokio::test]
    #[serial]
    async fn test_get_matched_order() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();
        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();
        assert_eq!(
            matched_order.unwrap().create_order.create_id,
            order.create_order.create_id
        );

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_matched_orders() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();
        let address = format!(
            "0x{}",
            alloy::primitives::hex::encode(alloy_primitives::Address::random())
        );
        let now = chrono::Utc::now();

        let order_config = TestMatchedOrderConfig {
            initiator_source_address: address.clone(),
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::InProgress])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        provider
            .update_swap_redeem(
                &order.destination_swap.swap_id,
                "0x1234567890123456789012345678901234567890",
                &order.source_swap.secret_hash,
                132,
                now,
            )
            .await
            .unwrap();

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();

        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        provider
            .update_swap_initiate(
                &order.source_swap.swap_id,
                BigDecimal::from_str("100").unwrap(),
                "0x1234567890123456789012345678901234567890",
                132,
                now,
            )
            .await
            .unwrap();

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::InProgress])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        provider
            .update_swap_refund(
                &order.source_swap.swap_id,
                "0x1234567890123456789012345678901234567890",
                132,
                now,
            )
            .await
            .unwrap();

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let matched_orders = provider
            .get_matched_orders(
                "0xjhknm",
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        let matched_orders = provider
            .get_matched_orders(
                &order.create_order.user_id.unwrap(),
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::InProgress])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 0);

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(1, 10, None, None, None, None, None, None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let order2 = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(1, 10, None, None, None, None, None, None, None).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 2);

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::InProgress])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        let matched_orders = provider
            .get_matched_orders(
                &address,
                OrderQueryFilters::new(
                    1,
                    10,
                    None,
                    None,
                    None,
                    None,
                    Some(HashSet::from([OrderStatusVerbose::Completed])),
                    None,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(matched_orders.data.len(), 1);

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&provider.pool, &order2.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_add_instant_refund_sacp() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        let instant_refund_sacp  = "02000000000101da620a354b0f3c6ad69dbd1daf4dc370a35d19e8e3433033ffba96f4bd80635d0000000000ffffffff01069b0700000000002251206c3525aba1c7f25c3390abf7565841c8dc8bfab2bb897b1adb3987599f63e3910441e6a0b47cb46944b8a8faacc6239fd8a8d4d7a34487cfed537c78b7ef15a6f51a11bd804cccd239e2bfa1506e404d4926b02598750499de75490773165cef6c258341e6a0b47cb46944b8a8faacc6239fd8a8d4d7a34487cfed537c78b7ef15a6f51a11bd804cccd239e2bfa1506e404d4926b02598750499de75490773165cef6c25834620712d22bac07e92f86bb6923620ceaa9c982d3d625133927cf41f52982e557c1cac20460f2e8ff81fc4e0a8e6ce7796704e3829e3e3eedb8db9390bdc51f4f04cf0a6ba529c61c12160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f005fe144a16b6e02d3f2362c042e4e7c639c21c7d2d46873ecc9309d6213da175b9386dc923e543c71cad95e7344c737eceaa7c5aec3d8b04c6ed0774708d442000000000".to_string();
        provider
            .add_instant_refund_sacp(&order.create_order.create_id, &instant_refund_sacp)
            .await
            .unwrap();

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();
        assert_eq!(
            matched_order
                .unwrap()
                .create_order
                .additional_data
                .instant_refund_tx_bytes
                .unwrap(),
            instant_refund_sacp
        );

        println!("Successfully added instant refund sacp");

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_add_redeem_sacp() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        let redeem_sacp = "02000000000101da620a354b0f3c6ad69dbd1daf4dc370a35d19e8e3433033ffba96f4bd80635d0000000000ffffffff01069b0700000000002251206c3525aba1c7f25c3390abf7565841c8dc8bfab2bb897b1adb3987599f63e3910441e6a0b47cb46944b8a8faacc6239fd8a8d4d7a34487cfed537c78b7ef15a6f51a11bd804cccd239e2bfa1506e404d4926b02598750499de75490773165cef6c258341e6a0b47cb46944b8a8faacc6239fd8a8d4d7a34487cfed537c78b7ef15a6f51a11bd804cccd239e2bfa1506e404d4926b02598750499de75490773165cef6c25834620712d22bac07e92f86bb6923620ceaa9c982d3d625133927cf41f52982e557c1cac20460f2e8ff81fc4e0a8e6ce7796704e3829e3e3eedb8db9390bdc51f4f04cf0a6ba529c61c12160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f005fe144a16b6e02d3f2362c042e4e7c639c21c7d2d46873ecc9309d6213da175b9386dc923e543c71cad95e7344c737eceaa7c5aec3d8b04c6ed0774708d442000000000".to_string();

        let redeem_tx_id =
            "380f681ba8541da5e6e8f256cdd3b564f7030c7b716d9dc37fe7b22136927d42".to_string();
        // passing secret_hash in place of secret.
        provider
            .add_redeem_sacp(
                &order.create_order.create_id,
                &redeem_sacp,
                &redeem_tx_id,
                &order.source_swap.secret_hash,
            )
            .await
            .unwrap();

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();

        assert_eq!(
            matched_order
                .unwrap()
                .create_order
                .additional_data
                .redeem_tx_bytes
                .unwrap(),
            redeem_sacp
        );

        println!("Successfully added redeem sacp");

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_pending_orders() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let filler = "bcd6f4cfa96358c74dbc03fec5ba25da66bbc92a31b714ce339dd93db1a9ffac";

        let source_chain_id = "bitcoin_regtest";
        let destination_chain_id = "ethereum_localnet";

        let order_config = TestMatchedOrderConfig {
            destination_chain_initiator_address: filler.to_string(),
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        // Chain as destination: check for initiation test.
        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        let pending_orders = provider
            .get_filler_pending_orders(destination_chain_id, filler)
            .await
            .unwrap();
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order.create_order.create_id));

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();

        // Chain as source : check for redeem.
        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();
        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&provider.pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();
        // note : here we are simulating the redeem for the destination swap and placing secret hash in place of secret.
        simulate_test_swap_redeem(
            &provider.pool,
            &order.destination_swap.swap_id,
            &order.source_swap.secret_hash,
            None,
        )
        .await
        .unwrap();
        let pending_orders = provider
            .get_filler_pending_orders(source_chain_id, filler)
            .await
            .unwrap();
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order.create_order.create_id));

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();

        // Refund case for either chain
        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&provider.pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();

        let pending_orders = provider
            .get_filler_pending_orders(source_chain_id, filler)
            .await
            .unwrap();
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order.create_order.create_id));

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();

        // Instant refund case when source chain matches
        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();

        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();

        let pending_orders = provider
            .get_filler_pending_orders(source_chain_id, filler)
            .await
            .unwrap();

        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order.create_order.create_id));

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();

        let order = create_test_matched_order(&provider.pool, order_config.clone())
            .await
            .unwrap();
        simulate_test_swap_initiate(&provider.pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&provider.pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_refund(&provider.pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();
        let pending_orders = provider
            .get_filler_pending_orders(source_chain_id, filler)
            .await
            .unwrap();
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order.create_order.create_id));

        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_solver_pending_orders() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let order_config1 = TestMatchedOrderConfig {
            source_chain: "starknet_devnet".to_string(),
            destination_chain: "bitcoin_regtest".to_string(),
            ..Default::default()
        };
        let order1 = create_test_matched_order(&provider.pool, order_config1)
            .await
            .unwrap();

        // Simulate swap initiate for the source swap on Order 1
        // This makes Order 1 pending for destination initiate
        simulate_test_swap_initiate(&provider.pool, &order1.source_swap.swap_id, None)
            .await
            .unwrap();

        let pending_orders = provider.get_solver_pending_orders().await.unwrap();
        assert_eq!(pending_orders.len(), 1);
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order1.create_order.create_id));

        let order_config2 = TestMatchedOrderConfig {
            source_chain: "arbitrum_localnet".to_string(),
            destination_chain: "bitcoin_regtest".to_string(),
            ..Default::default()
        };
        let order2 = create_test_matched_order(&provider.pool, order_config2)
            .await
            .unwrap();

        // Simulate swap initiate for the source swap on Order 2
        // This makes Order 2 pending for destination initiate
        simulate_test_swap_initiate(&provider.pool, &order2.source_swap.swap_id, None)
            .await
            .unwrap();

        let pending_orders = provider.get_solver_pending_orders().await.unwrap();
        assert_eq!(pending_orders.len(), 2);
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order2.create_order.create_id));

        // Simulate swap initiate for the destination swap on Order 2
        // Also simulate redeems on both swaps on Order 2
        // This makes order 2 complete
        simulate_test_swap_initiate(&provider.pool, &order2.destination_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order2.destination_swap.swap_id,
            &order2.source_swap.secret_hash,
            None,
        )
        .await
        .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order2.source_swap.swap_id,
            &order2.destination_swap.secret_hash,
            None,
        )
        .await
        .unwrap();

        let pending_orders = provider.get_solver_pending_orders().await.unwrap();
        assert_eq!(pending_orders.len(), 1);
        assert!(pending_orders
            .iter()
            .any(|o| o.create_order.create_id == order1.create_order.create_id));

        // Simulate refund on source swap on Order 1
        // This makes order 1 complete and not pending
        simulate_test_swap_refund(&provider.pool, &order1.source_swap.swap_id, None)
            .await
            .unwrap();

        let pending_orders = provider.get_solver_pending_orders().await.unwrap();
        assert_eq!(pending_orders.len(), 0);

        delete_matched_order(&provider.pool, &order1.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&provider.pool, &order2.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_update_swap_initiate() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let now = chrono::Utc::now();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        provider
            .update_swap_initiate(
                &order.source_swap.swap_id,
                BigDecimal::from_str("100").unwrap(),
                "0x1234567890123456789012345678901234567890",
                132,
                now,
            )
            .await
            .unwrap();

        let order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();

        let matched_order = order.unwrap();

        assert_eq!(
            matched_order.source_swap.filled_amount,
            BigDecimal::from_str("100").unwrap()
        );
        assert_eq!(
            matched_order.source_swap.initiate_tx_hash.to_string(),
            "0x1234567890123456789012345678901234567890".to_string()
        );
        assert_eq!(
            matched_order.source_swap.initiate_block_number.unwrap(),
            BigDecimal::from_str("132").unwrap()
        );
        assert_eq!(matched_order.source_swap.initiate_timestamp.unwrap(), now);

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_update_swap_redeem() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let now = chrono::Utc::now();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        let secret_hash = order.source_swap.secret_hash;

        // passing secret_hash in place of secret.
        provider
            .update_swap_redeem(
                &order.source_swap.swap_id,
                "0x1234567890123456789012345678901234567890",
                &secret_hash,
                132,
                now,
            )
            .await
            .unwrap();

        let order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();

        let matched_order = order.unwrap();

        assert_eq!(matched_order.source_swap.secret_hash, secret_hash);
        assert_eq!(
            matched_order.source_swap.redeem_tx_hash.to_string(),
            "0x1234567890123456789012345678901234567890".to_string()
        );
        assert_eq!(
            matched_order.source_swap.redeem_block_number.unwrap(),
            BigDecimal::from_str("132").unwrap()
        );
        assert_eq!(matched_order.source_swap.redeem_timestamp.unwrap(), now);

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_update_swap_refund() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let now = chrono::Utc::now();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        provider
            .update_swap_refund(
                &order.source_swap.swap_id,
                "0x1234567890123456789012345678901234567890",
                132,
                now,
            )
            .await
            .unwrap();

        let order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();

        let matched_order = order.unwrap();

        assert_eq!(
            matched_order.source_swap.refund_tx_hash.to_string(),
            "0x1234567890123456789012345678901234567890".to_string()
        );
        assert_eq!(
            matched_order.source_swap.refund_block_number.unwrap(),
            BigDecimal::from_str("132").unwrap()
        );
        assert_eq!(matched_order.source_swap.refund_timestamp.unwrap(), now);

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_update_confirmations() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let order_config = TestMatchedOrderConfig {
            ..Default::default()
        };
        let order = create_test_matched_order(&provider.pool, order_config)
            .await
            .unwrap();

        simulate_test_swap_initiate(
            &provider.pool,
            &order.source_swap.swap_id,
            Some(TestTxData {
                tx_hash: "0x1234567890123456789012345678901234567890".to_string(),
                block_number: 132,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 0,
                timestamp: chrono::Utc::now(),
            }),
        )
        .await
        .unwrap();
        provider
            .update_confirmations(&order.source_swap.chain, 133)
            .await
            .unwrap();

        let order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap();

        let matched_order = order.unwrap();

        assert_eq!(matched_order.source_swap.current_confirmations, 2);

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_volume() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();
        let mut asset_decimals = HashMap::new();

        asset_decimals.insert(("bitcoin_regtest".to_string(), "primary".to_string()), 8);
        asset_decimals.insert(
            (
                "ethereum_localnet".to_string(),
                "0x0165878A594ca255338adfa4d48449f69242Eb8F"
                    .to_string()
                    .to_lowercase(),
            ),
            8,
        );

        // Create test orders with known amounts and prices
        let order_config1 = TestMatchedOrderConfig {
            source_amount: BigDecimal::from(1000000000000i64),
            destination_amount: BigDecimal::from(500000000000i64),
            ..Default::default()
        };

        let order1 = create_test_matched_order(&provider.pool, order_config1)
            .await
            .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order1.source_swap.swap_id,
            &order1.destination_swap.secret_hash,
            None,
        )
        .await
        .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order1.destination_swap.swap_id,
            &order1.source_swap.secret_hash,
            None,
        )
        .await
        .unwrap();

        // Test 1: Get all volume
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(15000));

        // Test 2: Get volume for specific chain
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: Some("ethereum_localnet".to_string()),
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(5000));

        // sleep for a second to ensure different timestamps
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // Test 3: Get volume with time interval
        let now = Utc::now().timestamp();
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: Some(now),
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(15000));

        // Test 4 : Get volume for source and destination chain
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: Some("arbitrum_localnet".to_string()),
                    destination_chain: Some("ethereum_localnet".to_string()),
                    address: None,
                    from: None,
                    to: Some(now),
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(0));

        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: Some("bitcoin_regtest".to_string()),
                    destination_chain: Some("ethereum_localnet".to_string()),
                    address: None,
                    from: None,
                    to: Some(now),
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(15000));

        // Test 5 : Get volume for with from time interval
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: Some(now),
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(0));

        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: Some(order1.create_order.created_at.timestamp()),
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(15000));

        // Test 6 : Get volume based on the given address
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: Some("bitcoin_regtest".to_string()),
                    destination_chain: None,
                    address: Some(order1.source_swap.initiator.to_string()),
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(10000));

        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: Some("0x0000000000000000000000000000000000000000".to_string()),
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(0));

        // Test 7 : Get volume for all filters
        let volume = provider
            .get_volume(
                StatsQueryFilters {
                    source_chain: Some("bitcoin_regtest".to_string()),
                    destination_chain: Some("ethereum_localnet".to_string()),
                    address: Some(order1.destination_swap.redeemer.to_string()),
                    from: Some(order1.create_order.created_at.timestamp()),
                    to: Some(now),
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(volume, BigDecimal::from(15000));

        // Clean up
        delete_matched_order(&provider.pool, &order1.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_fees() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();
        let mut asset_decimals = HashMap::new();

        // Setup test data with known decimals
        asset_decimals.insert(("bitcoin_regtest".to_string(), "primary".to_string()), 8);
        asset_decimals.insert(
            (
                "ethereum_localnet".to_string(),
                "0x0165878A594ca255338adfa4d48449f69242Eb8F"
                    .to_string()
                    .to_lowercase(),
            ),
            8,
        );

        // Create test orders with known
        let order_config1 = TestMatchedOrderConfig {
            source_amount: BigDecimal::from(1000000000000i64),
            destination_amount: BigDecimal::from(500000000000i64),
            ..Default::default()
        };

        let order1 = create_test_matched_order(&provider.pool, order_config1)
            .await
            .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order1.source_swap.swap_id,
            &order1.destination_swap.secret_hash,
            None,
        )
        .await
        .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order1.destination_swap.swap_id,
            &order1.source_swap.secret_hash,
            None,
        )
        .await
        .unwrap();

        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(5000));

        // Create another order to test fees calculation
        let order_config2 = TestMatchedOrderConfig {
            source_amount: BigDecimal::from(1000000000000i64),
            destination_amount: BigDecimal::from(500000000000i64),
            ..Default::default()
        };

        let order2 = create_test_matched_order(&provider.pool, order_config2)
            .await
            .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order2.source_swap.swap_id,
            &order2.destination_swap.secret_hash,
            None,
        )
        .await
        .unwrap();
        simulate_test_swap_redeem(
            &provider.pool,
            &order2.destination_swap.swap_id,
            &order2.source_swap.secret_hash,
            None,
        )
        .await
        .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // Test 2: Get all fees (should now include $5000 from second order)
        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(10000));

        // Test 3: Get fees for specific chain
        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: Some("ethereum_localnet".to_string()),
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(0));

        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: Some("bitcoin_regtest".to_string()),
                    destination_chain: None,
                    address: None,
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(10000));

        // Test 4 : Get fees for given address
        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: Some(order1.destination_swap.redeemer.to_string()),
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(5000));

        let fees = provider
            .get_fees(
                StatsQueryFilters {
                    source_chain: None,
                    destination_chain: None,
                    address: Some("0x0000000000000000000000000000000000000000".to_string()),
                    from: None,
                    to: None,
                },
                &asset_decimals,
            )
            .await
            .unwrap();

        assert_eq!(fees, BigDecimal::from(0));

        // Clean up
        delete_matched_order(&provider.pool, &order1.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&provider.pool, &order2.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_create_matched_order() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();
        let mut order = default_matched_order();
        order.source_swap.htlc_address = Some("source_swap_htlc_address".to_string());
        order.destination_swap.htlc_address = Some("destination_swap_htlc_address".to_string());
        order.source_swap.token_address = Some("destination_swap_htlc_address".to_string());
        order.destination_swap.token_address = Some("destination_swap_token_address".to_string());
        provider.create_matched_order(&order).await.unwrap();

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            matched_order.create_order.create_id,
            order.create_order.create_id
        );
        let res = provider.create_matched_order(&order).await;
        assert!(res.is_err());
        assert!(matches!(
            res.err(),
            Some(OrderbookError::OrderAlreadyExists(_))
        ));

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            matched_order.create_order.create_id,
            order.create_order.create_id
        );
        assert_eq!(
            order.source_swap.htlc_address,
            matched_order.source_swap.htlc_address
        );
        assert_eq!(
            order.destination_swap.htlc_address,
            matched_order.destination_swap.htlc_address
        );
        assert_eq!(
            order.source_swap.token_address,
            matched_order.source_swap.token_address
        );
        assert_eq!(
            order.destination_swap.token_address,
            matched_order.destination_swap.token_address
        );

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_create_matched_order_preserves_solana_address_case() {
        let provider = provider().await;
        delete_all_matched_orders(&provider.pool).await.unwrap();

        let sol_initiator = "6zo8pUmpTisTKp19goQEDPNbmyxf6FtEXaGJnmcgaqbU".to_string();
        let sol_redeemer = "GuTqTajbyF4d6F7CRWJApiensETs2sDPHK9BXMEV3yjt".to_string();

        let mut order = default_matched_order();
        order.source_swap.swap_id = "solana-case-source".to_string();
        order.destination_swap.swap_id = "solana-case-destination".to_string();
        order.create_order.create_id = "solana-case-create-order".to_string();

        order.source_swap.chain = "solana_devnet".to_string();
        order.source_swap.asset = "primary".to_string();
        order.source_swap.htlc_address = Some("primary".to_string());
        order.source_swap.token_address = Some("primary".to_string());
        order.source_swap.initiator = sol_initiator.clone();
        order.source_swap.redeemer = sol_redeemer.clone();

        order.destination_swap.chain = "tars_1".to_string();
        order.destination_swap.asset = "primary".to_string();
        order.destination_swap.htlc_address = Some("primary".to_string());
        order.destination_swap.token_address = Some("primary".to_string());

        order.create_order.source_chain = "solana_devnet".to_string();
        order.create_order.destination_chain = "tars_1".to_string();
        order.create_order.source_asset = "primary".to_string();
        order.create_order.destination_asset = "primary".to_string();
        order.create_order.initiator_source_address = sol_initiator.clone();

        provider.create_matched_order(&order).await.unwrap();

        let matched_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(matched_order.source_swap.initiator, sol_initiator);
        assert_eq!(matched_order.source_swap.redeemer, sol_redeemer);
        assert_eq!(
            matched_order.create_order.initiator_source_address,
            order.create_order.initiator_source_address
        );

        delete_matched_order(&provider.pool, &matched_order.create_order.create_id)
            .await
            .unwrap();
    }
}
