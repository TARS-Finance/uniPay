use crate::primitives::{ActionType, ActionWithInfo, MatchedOrderVerbose, SingleSwap};
use bigdecimal::{BigDecimal, FromPrimitive, Zero};
use chrono::Utc;
use eyre::Result;
use fiat::fiat_fetcher::FiatProvider;
use primitives::HTLCAction;
use std::{collections::HashMap, time::Duration};
use utils::ToBytes;

/// Default price drop threshold is 100 bips
const DEFAULT_PRICE_DROP_THRESHOLD: f64 = 0.01;

/// Default minimum refundable duration
const DEFAULT_MIN_REFUNDABLE_DURATION: i64 = 90;

/// OrderMapper is responsible for mapping the order to an HTLC action
#[derive(Debug, Clone)]
pub struct OrderMapper {
    /// The fiat provider to use for price fetching
    fiat_provider: FiatProvider,
    /// The price drop threshold to use for price validation
    price_drop_threshold: f64,
    /// The chains to check for valid action
    supported_chains: Vec<String>,
    /// The additional checks to perform on the order for a given action
    additional_checks: HashMap<ActionType, fn(&MatchedOrderVerbose) -> bool>,
}

/// Builder for OrderMapper to allow flexible construction
#[derive(Debug, Clone)]
pub struct OrderMapperBuilder {
    fiat_provider: FiatProvider,
    price_drop_threshold: Option<f64>,
    supported_chains: Vec<String>,
    additional_checks: HashMap<ActionType, fn(&MatchedOrderVerbose) -> bool>,
}

impl OrderMapperBuilder {
    /// Create a new OrderMapperBuilder with the required fiat provider
    ///
    /// # Arguments
    /// * `fiat_provider` - The fiat provider to use for price fetching
    pub fn new(fiat_provider: FiatProvider) -> Self {
        Self {
            fiat_provider,
            price_drop_threshold: None,
            supported_chains: Vec::new(),
            additional_checks: HashMap::new(),
        }
    }

    /// Set the price drop threshold
    ///
    /// # Arguments
    /// * `threshold` - The price drop threshold to use
    pub fn with_price_drop_threshold(mut self, threshold: f64) -> Self {
        self.price_drop_threshold = Some(threshold);
        self
    }

    /// Add a supported chain
    ///
    /// # Arguments
    /// * `chain` - The chain to add
    pub fn add_supported_chain(mut self, chain: String) -> Self {
        self.supported_chains.push(chain);
        self
    }

    /// Set all supported chains
    ///
    /// # Arguments
    /// * `chains` - The chains to support
    pub fn with_supported_chains(mut self, chains: Vec<String>) -> Self {
        self.supported_chains = chains;
        self
    }

    /// Add an additional check for an action
    ///
    /// # Arguments
    /// * `action_type` - The action type to check
    /// * `check` - The check function
    pub fn add_check(
        mut self,
        action_type: ActionType,
        check: fn(&MatchedOrderVerbose) -> bool,
    ) -> Self {
        self.additional_checks.insert(action_type, check);
        self
    }

    /// Set all additional checks
    ///
    /// # Arguments
    /// * `checks` - The checks to perform
    pub fn with_additional_checks(
        mut self,
        checks: HashMap<ActionType, fn(&MatchedOrderVerbose) -> bool>,
    ) -> Self {
        self.additional_checks = checks;
        self
    }

    /// Build the OrderMapper
    pub fn build(self) -> OrderMapper {
        OrderMapper::new(
            self.fiat_provider,
            self.price_drop_threshold,
            self.supported_chains,
            self.additional_checks,
        )
    }
}

impl OrderMapper {
    /// Create a new OrderMapper
    ///
    /// # Arguments
    /// * `fiat_provider` - The fiat provider to use for price fetching
    /// * `price_drop_threshold` - The price drop threshold to use for price validation
    /// * `supported_chains` - The chains to check for valid action
    /// * `additional_checks` - The additional checks to perform on the order for a given action
    pub fn new(
        fiat_provider: FiatProvider,
        price_drop_threshold: Option<f64>,
        supported_chains: Vec<String>,
        additional_checks: HashMap<ActionType, fn(&MatchedOrderVerbose) -> bool>,
    ) -> Self {
        let price_drop_threshold = price_drop_threshold.unwrap_or(DEFAULT_PRICE_DROP_THRESHOLD);
        Self {
            fiat_provider,
            price_drop_threshold,
            supported_chains,
            additional_checks,
        }
    }

    /// Create a new OrderMapperBuilder
    ///
    /// # Arguments
    /// * `fiat_provider` - The fiat provider to use for price fetching
    pub fn builder(fiat_provider: FiatProvider) -> OrderMapperBuilder {
        OrderMapperBuilder::new(fiat_provider)
    }

    /// Map the order to an action
    ///
    /// # Arguments
    /// * `order` - The order to map
    /// * `latest_dest_chain_block_number` - The latest block number on the destination chain
    ///
    /// # Returns
    /// * `Result<ActionWithInfo>` - The action to perform and the swap details
    pub async fn map(
        &self,
        order: &MatchedOrderVerbose,
        latest_dest_chain_block_number: Option<&BigDecimal>,
    ) -> Result<ActionWithInfo> {
        // Check destination chain actions first
        if let Some(action) = self
            .check_destination_chain_actions(order, latest_dest_chain_block_number)
            .await?
        {
            return Ok(action);
        }

        // Check source chain actions
        if let Some(action) = self.check_source_chain_actions(order).await? {
            return Ok(action);
        }

        Ok(ActionWithInfo {
            action: HTLCAction::NoOp,
            swap: None,
        })
    }

    /// Check actions that can be performed on the source chain
    async fn check_source_chain_actions(
        &self,
        order: &MatchedOrderVerbose,
    ) -> Result<Option<ActionWithInfo>> {
        // Check if the source chain of the order is supported
        // if yes , we check for the action that can be performed on the source chain
        // Actions that can be performed on the source chain
        // 1. Instant Refund
        // 2. Redeem
        if self.is_chain_supported(&order.source_swap.chain) {
            let swap = order.source_swap.clone();

            // Check redeem first
            if self.can_redeem(order) {
                // We know secret is Some because can_redeem checks for it
                let secret = order.destination_swap.secret.to_string().hex_to_bytes()?;
                return Ok(Some(ActionWithInfo {
                    action: HTLCAction::Redeem { secret },
                    swap: Some(swap),
                }));
            }

            // Try instant refund
            if self.can_instant_refund(order).await? {
                return Ok(Some(ActionWithInfo {
                    action: HTLCAction::InstantRefund,
                    swap: Some(swap),
                }));
            }
        }

        Ok(None)
    }

    /// Check actions that can be performed on the destination chain
    async fn check_destination_chain_actions(
        &self,
        order: &MatchedOrderVerbose,
        latest_dest_chain_block_number: Option<&BigDecimal>,
    ) -> Result<Option<ActionWithInfo>> {
        // Check if the destination chain of the order is supported
        // if yes , we check for the action that can be performed on the destination chain
        // Actions that can be performed on the destination chain
        // 1. Refund
        // 2. Initiate
        if self.is_chain_supported(&order.destination_swap.chain) {
            let swap = order.destination_swap.clone();

            // Only check for refund if we have a valid block number
            if let Some(block_number) = latest_dest_chain_block_number {
                if self.can_refund(order, block_number) {
                    return Ok(Some(ActionWithInfo {
                        action: HTLCAction::Refund,
                        swap: Some(swap),
                    }));
                }
            }

            // Try initiate
            if self.can_initiate(order).await? {
                return Ok(Some(ActionWithInfo {
                    action: HTLCAction::Initiate,
                    swap: Some(swap),
                }));
            }
        }

        Ok(None)
    }

    /// Check if a chain is supported
    fn is_chain_supported(&self, chain: &str) -> bool {
        self.supported_chains.contains(&chain.to_string())
    }

    /// Check if a swap is already completed (redeemed or refunded)
    fn is_swap_completed(&self, swap: &SingleSwap) -> bool {
        self.has_refund(swap) || self.has_redeem(swap)
    }

    /// Check if a BigDecimal block number is valid (exists and > 0)
    fn is_valid_block_number(&self, block_number: &Option<BigDecimal>) -> bool {
        block_number
            .as_ref()
            .map_or(false, |block| block > &BigDecimal::zero())
    }

    /// Check if the order can be instant refunded
    ///
    /// # Arguments
    /// * `order` - The order to check
    ///
    /// # Returns
    /// * `Result<bool>` - Whether the order is an instant refund
    async fn can_instant_refund(&self, order: &MatchedOrderVerbose) -> Result<bool> {
        let source_swap = &order.source_swap;
        let destination_swap = &order.destination_swap;

        // Check additional checks if configured
        let additional_check_passes = self
            .additional_checks
            .get(&ActionType::InstantRefund)
            .map_or(true, |check| check(order));

        if !additional_check_passes {
            return Ok(false);
        }

        // Source swap must have valid initiate and not be refunded
        if !self.has_valid_initiate(source_swap) || self.has_refund(source_swap) {
            return Ok(false);
        }

        // Destination swap must not have initiate without refund
        if destination_swap.initiate_tx_hash.is_some() && !self.is_swap_completed(destination_swap)
        {
            return Ok(false);
        }

        // If the swap is partially filled, instant refund to user
        if source_swap.amount != source_swap.filled_amount {
            return Ok(true);
        }

        // Destination must not be redeemed
        if self.has_redeem(&order.destination_swap) {
            return Ok(false);
        }

        // Check specific instant refund conditions
        if self.has_destination_refund_only(order) {
            return Ok(true);
        }

        if self.is_deadline_expired(order) {
            return Ok(true);
        }

        // Check price threshold (most expensive check last)
        let is_price_within_threshold = self.check_price_threshold(order).await?;
        Ok(!is_price_within_threshold)
    }

    /// Check if the destination has a refund but source doesn't
    fn has_destination_refund_only(&self, order: &MatchedOrderVerbose) -> bool {
        let destination_swap = &order.destination_swap;
        let source_swap = &order.source_swap;

        destination_swap.refund_tx_hash.is_some()
            && self.is_valid_block_number(&destination_swap.refund_block_number)
            && source_swap.refund_tx_hash.is_none()
    }

    fn has_redeem(&self, swap: &SingleSwap) -> bool {
        swap.redeem_tx_hash.is_some() && swap.secret.is_some()
    }

    fn has_refund(&self, swap: &SingleSwap) -> bool {
        swap.refund_tx_hash.is_some()
    }

    /// Check if the order deadline has expired
    fn is_deadline_expired(&self, order: &MatchedOrderVerbose) -> bool {
        let current_time = chrono::Utc::now();
        let deadline =
            chrono::DateTime::from_timestamp(order.create_order.additional_data.deadline, 0)
                .unwrap_or(order.created_at + Duration::from_secs(60 * 60));
        current_time > deadline
    }

    /// Check if price is within threshold
    async fn check_price_threshold(&self, order: &MatchedOrderVerbose) -> Result<bool> {
        let order_pair = order.get_order_pair();
        let (input_price, output_price) = self.fiat_provider.get_price(&order_pair).await?;

        let additional_data = &order.create_order.additional_data;
        let (order_input_price, order_output_price) = (
            additional_data.input_token_price,
            additional_data.output_token_price,
        );

        Ok(Self::validate_price_threshold(
            order_input_price,
            order_output_price,
            input_price,
            output_price,
            self.price_drop_threshold,
        ))
    }

    /// Check if the order can be initiated
    ///
    /// # Arguments
    /// * `order` - The order to check
    ///
    /// # Returns
    /// * `Result<bool>` - Whether the order is an initiate
    async fn can_initiate(&self, order: &MatchedOrderVerbose) -> Result<bool> {
        // Don't initiate if blacklisted
        if order.create_order.additional_data.is_blacklisted {
            return Ok(false);
        }

        let source_swap = &order.source_swap;
        let is_source_bitcoin = source_swap.chain.contains("bitcoin");

        // Source swap must have an initiate tx. For non-bitcoin sources we also
        // require a valid block number and the configured confirmation count;
        // bitcoin sources can trigger destination initiate as soon as the
        // initiate tx is observed (0-conf), so we skip those checks.
        if is_source_bitcoin {
            if !source_swap.initiate_tx_hash.is_some() {
                return Ok(false);
            }
        } else {
            if !self.has_valid_initiate(source_swap) {
                return Ok(false);
            }
            if source_swap.current_confirmations < source_swap.required_confirmations {
                return Ok(false);
            }
        }

        // Destination swap must not have initiate tx hash
        if order.destination_swap.initiate_tx_hash.is_some() {
            return Ok(false);
        }

        // Source swap must have amount filled
        if source_swap.filled_amount != source_swap.amount {
            return Ok(false);
        }

        // Check additional validation
        if !self.passes_additional_check(ActionType::Initiate, order) {
            return Ok(false);
        }

        // Don't initiate if we can instant refund
        let can_instant_refund = self.can_instant_refund(order).await?;
        Ok(!can_instant_refund)
    }

    /// Check if the swap has valid initiate tx and block number
    fn has_valid_initiate(&self, swap: &SingleSwap) -> bool {
        swap.initiate_tx_hash.is_some()
            && swap
                .initiate_block_number
                .as_ref()
                .map_or(false, |block| block > &BigDecimal::zero())
    }

    /// Check if the order can be refunded
    ///
    /// # Arguments
    /// * `order` - The order to check
    /// * `latest_dest_chain_block_number` - The latest block number on the destination chain
    ///
    /// # Returns
    /// * `bool` - Whether the order is a refund
    fn can_refund(
        &self,
        order: &MatchedOrderVerbose,
        latest_dest_chain_block_number: &BigDecimal,
    ) -> bool {
        let destination_swap = &order.destination_swap;

        // Can't refund if already refunded
        if self.is_swap_completed(&destination_swap) {
            return false;
        }

        // Must have valid initiate block number
        if !self.is_valid_block_number(&destination_swap.initiate_block_number) {
            return false;
        }

        // Check additional validations
        if !self.passes_additional_check(ActionType::Refund, order) {
            return false;
        }

        // Get timelock as BigDecimal and check expiration
        let dest_init_block_num = destination_swap.initiate_block_number.as_ref().unwrap();
        let timelock = BigDecimal::from_i32(destination_swap.timelock).unwrap_or_default();
        latest_dest_chain_block_number >= &(dest_init_block_num + timelock)
    }

    /// Check if the order can be redeemed
    ///
    /// # Arguments
    /// * `order` - The order to check
    ///
    /// # Returns
    /// * `bool` - Whether the order can be redeemed
    fn can_redeem(&self, order: &MatchedOrderVerbose) -> bool {
        let source_swap = &order.source_swap;

        if self.is_swap_completed(source_swap) {
            return false;
        }

        // Source must have been initiated before we can redeem it
        if !self.has_valid_initiate(source_swap) {
            return false;
        }

        if !self.passes_additional_check(ActionType::Redeem, order) {
            return false;
        }

        // Destination must have been redeemed (secret populated by watcher after initia executor redeems)
        order.destination_swap.secret.is_some()
    }

    /// Validate the price threshold
    ///
    /// # Arguments
    /// * `original_input_price` - The original input price
    /// * `original_output_price` - The original output price
    /// * `current_input_price` - The current input price
    /// * `current_output_price` - The current output price
    /// * `price_drop_threshold` - The price drop threshold
    ///
    /// # Returns
    /// * `bool` - Whether the price is within the threshold
    #[inline]
    pub fn validate_price_threshold(
        original_input_price: f64,
        original_output_price: f64,
        current_input_price: f64,
        current_output_price: f64,
        price_drop_threshold: f64,
    ) -> bool {
        // Avoid division by zero
        if original_input_price <= 0.0 || original_output_price <= 0.0 {
            return false;
        }

        // When input price falls, system receives less value
        let input_price_decrease =
            (original_input_price - current_input_price) / original_input_price;

        // When output price rises, system pays more
        let output_price_increase =
            (current_output_price - original_output_price) / original_output_price;

        let combined_system_loss = input_price_decrease + output_price_increase;

        // Check both user and system value protection
        let user_value_protection =
            current_output_price >= original_output_price * (1.0 - price_drop_threshold);
        let system_value_protection = combined_system_loss <= price_drop_threshold;

        user_value_protection && system_value_protection
    }

    fn passes_additional_check(
        &self,
        action_type: ActionType,
        order: &MatchedOrderVerbose,
    ) -> bool {
        self.additional_checks
            .get(&action_type)
            .map_or(true, |check| check(order))
    }

    /// Check if any of the orders can be refunded
    ///
    /// Ensures that the destination swap is on a supported chain, has a valid initiate,
    /// is not completed, passes additional checks if any, and has passed the minimum refundable duration.
    /// Current minimum refundable duration is 90 minutes.
    ///
    /// # Arguments
    /// * `orders` - The orders to check
    ///
    /// # Returns
    /// * `bool` - Whether any of the orders are refundable
    pub fn has_refundable_swaps(&self, orders: &Vec<MatchedOrderVerbose>) -> bool {
        orders.iter().any(|order| {
            let destination_swap = &order.destination_swap;
            if !self.supported_chains.contains(&destination_swap.chain) {
                return false;
            }
            self.has_valid_initiate(&destination_swap)
                && !self.is_swap_completed(&destination_swap)
                && self.passes_additional_check(ActionType::Refund, order)
                && destination_swap.created_at
                    + chrono::Duration::minutes(DEFAULT_MIN_REFUNDABLE_DURATION)
                    <= Utc::now()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, str::FromStr};

    use crate::{
        order_mapper::DEFAULT_PRICE_DROP_THRESHOLD,
        primitives::{ActionType, MatchedOrderVerbose, MaybeString},
        test_utils::default_matched_order,
        OrderMapper,
    };
    use bigdecimal::BigDecimal;
    use chrono::Utc;
    use fiat::{test_utils::start_mock_fiat_server, FiatProvider};
    use primitives::HTLCAction;

    const PRICE_DROP_THRESHOLD: f64 = 0.1;

    use std::sync::OnceLock;
    static SERVER_URL: OnceLock<String> = OnceLock::new();

    async fn start_mock_server() -> &'static str {
        let url = start_mock_fiat_server().await;
        SERVER_URL.set(url).ok(); // only sets once
        SERVER_URL.get().unwrap()
    }

    #[tokio::test]
    async fn test_order_mapper() {
        let url = start_mock_server().await;

        let mut order = default_matched_order();
        order.source_swap.required_confirmations = 3;
        order.source_swap.filled_amount = BigDecimal::from_str("10000000").unwrap();

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        // Use the builder pattern to create the OrderMapper
        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec![
                "arbitrum_localnet".to_string(),
                "bitcoin_regtest".to_string(),
            ])
            .build();

        // Test 1: No action when order is just created
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "New order should have no action"
        );

        // Mock User Initiate
        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.filled_amount = BigDecimal::from_str("10000").unwrap();
        order.source_swap.current_confirmations = 2i32;

        // Confirmations not reached
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should trigger No action when current confirmations are less than required confirmations"
        );

        // Confirmations reached
        order.source_swap.current_confirmations = 3i32;

        order.create_order.additional_data.deadline = Utc::now().timestamp() - 7200; // Set deadline to 2 hours ago
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund on deadline expiry"
        );

        // Reset deadline for next tests
        order.create_order.additional_data.deadline = Utc::now().timestamp() + 3600;
        order.create_order.additional_data.instant_refund_tx_bytes =
            Some("instant_refund_bytes".to_string());

        order.source_swap.filled_amount = BigDecimal::from(0);
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund with partial filled amount"
        );

        order.source_swap.filled_amount = order.source_swap.amount.clone();
        order.source_swap.initiate_block_number = Some(BigDecimal::from(0u32));
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate without valid initiate block number"
        );

        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Initiate),
            "Should trigger initiate with valid price threshold"
        );

        order.create_order.additional_data.output_token_price = 100.0;
        order.create_order.additional_data.input_token_price = 100.0;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund when price threshold is not met"
        );

        // Reset prices for next test   s
        order.create_order.additional_data.output_token_price = 1.0;
        order.create_order.additional_data.input_token_price = 1.0;

        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.secret = MaybeString::new("0x736563726574".to_string());
        order.source_swap.redeem_tx_hash = MaybeString::new("".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
             matches!(mapper_result.action, HTLCAction::Redeem { secret: _ }),
             "Should trigger redeem action when destination has secret and source redeem tx hash is empty"
         );

        // Reset order for next test
        order.destination_swap.secret = MaybeString::new("".to_string());

        order.destination_swap.initiate_tx_hash = MaybeString::new("".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(4u32));
        order.source_swap.required_confirmations = 3;
        order.source_swap.current_confirmations = 2;
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should trigger No action when source swap current confirmations are less than required confirmations"
        );

        order.destination_swap.refund_tx_hash = MaybeString::new("0xtx_refund".to_string());
        order.destination_swap.refund_block_number = Some(BigDecimal::from(1u32));
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund when destination has refund"
        );

        // Reset order for next test
        order.destination_swap.refund_tx_hash = MaybeString::new("".to_string());

        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.destination_swap.timelock = 1;
        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Refund),
            "Should trigger refund action"
        );

        // Reset order for next test
        order.destination_swap.initiate_tx_hash = MaybeString::new("".to_string());
        order.destination_swap.initiate_block_number = None;

        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 1;
        order.source_swap.current_confirmations = 1;
        order.create_order.additional_data.deadline = Utc::now().timestamp() + 3600;
        // Add price data for validation
        order.create_order.additional_data.input_token_price = 1.0;
        order.create_order.additional_data.output_token_price = 1.0;
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Initiate),
            "Should trigger initiate action"
        );
    }

    fn mock_check(order: &MatchedOrderVerbose) -> bool {
        order.create_order.additional_data.redeem_tx_bytes.is_some()
    }

    #[tokio::test]
    async fn test_order_mapper_with_additional_checks() {
        let url = start_mock_server().await;

        let mut order = default_matched_order();

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let mut additional_checks: HashMap<ActionType, fn(&MatchedOrderVerbose) -> bool> =
            HashMap::new();
        additional_checks.insert(ActionType::InstantRefund, mock_check);
        additional_checks.insert(ActionType::Refund, mock_check);
        additional_checks.insert(ActionType::Initiate, mock_check);
        additional_checks.insert(ActionType::Redeem, mock_check);

        // Use the builder pattern to create the OrderMapper with additional checks
        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec![
                "arbitrum_localnet".to_string(),
                "bitcoin_regtest".to_string(),
            ])
            .with_additional_checks(additional_checks)
            .build();

        // simulate source swap initiate
        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 1;
        order.source_swap.current_confirmations = 1;
        order.source_swap.filled_amount = order.source_swap.amount.clone();
        order.create_order.additional_data.deadline = Utc::now().timestamp() + 3600;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action when additional check fails"
        );

        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.create_order.additional_data.redeem_tx_bytes = Some("redeem_bytes".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Initiate),
            "Should trigger initiate action when additional check passes"
        );

        // simulate destination swap initiate
        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1000000u32));
        order.destination_swap.required_confirmations = 1;
        order.destination_swap.current_confirmations = 1;
        order.destination_swap.filled_amount = order.destination_swap.amount.clone();

        // simulate destination swap redeem
        order.destination_swap.redeem_tx_hash = MaybeString::new("0xtx_redeem".to_string());
        order.destination_swap.secret = MaybeString::new("0x736563726574".to_string());

        order.create_order.additional_data.redeem_tx_bytes = None;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger redeem action when additional check fails"
        );

        order.create_order.additional_data.redeem_tx_bytes = Some("redeem_bytes".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Redeem { secret: _ }),
            "Should trigger redeem action when additional check passes"
        );

        // reset redeem
        order.destination_swap.redeem_tx_hash = MaybeString::new("".to_string());
        order.destination_swap.secret = MaybeString::new("".to_string());

        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.destination_swap.required_confirmations = 1;
        order.destination_swap.current_confirmations = 1;
        order.destination_swap.filled_amount = order.destination_swap.amount.clone();

        order.create_order.additional_data.redeem_tx_bytes = None;

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger refund action when additional check fails"
        );

        order.create_order.additional_data.redeem_tx_bytes = Some("redeem_bytes".to_string());
        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Refund),
            "Should trigger refund action when additional check passes"
        );

        // simulate destination swap refund
        order.destination_swap.refund_tx_hash = MaybeString::new("0xtx_refund".to_string());
        order.destination_swap.refund_block_number = Some(BigDecimal::from(1u32));
        order.create_order.additional_data.redeem_tx_bytes = None;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger instant refund action when additional check fails"
        );

        order.create_order.additional_data.redeem_tx_bytes = Some("redeem_bytes".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund action when additional check passes"
        );
    }

    #[tokio::test]
    async fn test_builder_pattern() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        // Test builder with minimal configuration
        let minimal_mapper = OrderMapper::builder(fiat_provider.clone()).build();
        assert_eq!(
            minimal_mapper.price_drop_threshold,
            DEFAULT_PRICE_DROP_THRESHOLD
        );
        assert!(minimal_mapper.supported_chains.is_empty());
        assert!(minimal_mapper.additional_checks.is_empty());

        // Test builder with complete configuration
        let complete_mapper = OrderMapper::builder(fiat_provider.clone())
            .with_price_drop_threshold(0.2)
            .add_supported_chain("arbitrum".to_string())
            .add_supported_chain("bitcoin".to_string())
            .add_check(ActionType::Redeem, mock_check)
            .build();

        assert_eq!(complete_mapper.price_drop_threshold, 0.2);
        assert_eq!(complete_mapper.supported_chains.len(), 2);
        assert_eq!(complete_mapper.additional_checks.len(), 1);
    }

    #[tokio::test]
    async fn test_mapper_redeem_case() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec!["arbitrum_localnet".to_string()])
            .build();

        let mut order = default_matched_order();

        // simulate source swap initiate
        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 1;
        order.source_swap.current_confirmations = 1;
        order.source_swap.filled_amount = order.source_swap.amount.clone();

        // simulate destination swap initiate
        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1000000u32));
        order.destination_swap.required_confirmations = 1;
        order.destination_swap.current_confirmations = 1;
        order.destination_swap.filled_amount = order.destination_swap.amount.clone();

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger redeem action when Destination swap doesnt have secret"
        );

        order.destination_swap.secret = MaybeString::new("0x736563726574".to_string());
        order.destination_swap.redeem_tx_hash = MaybeString::new("0xtx_redeem".to_string());

        order.source_swap.refund_tx_hash = MaybeString::new("0xtx_refund".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger redeem action when Source swap is refunded"
        );

        order.source_swap.refund_tx_hash = MaybeString::new("".to_string());
        order.source_swap.redeem_tx_hash = MaybeString::new("0xtx_redeem".to_string());
        order.source_swap.secret = MaybeString::new("0x736563726574".to_string());
        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger redeem action when Source swap is already redeemed"
        );

        order.source_swap.redeem_tx_hash = MaybeString::new("".to_string());

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Redeem { secret: _ }),
            "Should trigger redeem action when Destination swap has secret and Source swap is not redeemed or refunded"
        );
    }

    #[tokio::test]
    async fn test_mapper_refund_case() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec![
                "arbitrum_localnet".to_string(),
                "bitcoin_regtest".to_string(),
            ])
            .build();

        let mut order = default_matched_order();
        order.destination_swap.timelock = 1;

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger refund action when destination swap is not initiated"
        );

        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(0u32));

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger refund action without a valid initiate block"
        );

        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.destination_swap.refund_tx_hash = MaybeString::new("0xtx_refund".to_string());

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger refund action when destination swap is already refunded"
        );

        order.destination_swap.redeem_tx_hash = MaybeString::new("0xtx_redeem".to_string());
        order.destination_swap.secret = MaybeString::new("0x736563726574".to_string());
        // this skips the redeem action
        order.source_swap.refund_tx_hash = MaybeString::new("0xtx_refund".to_string());

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger refund action when destination swap is already redeemed"
        );

        order.destination_swap.refund_tx_hash = MaybeString::new("".to_string());
        order.destination_swap.secret = MaybeString::new("".to_string());
        order.destination_swap.redeem_tx_hash = MaybeString::new("".to_string());

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should trigger no action when on refund case but destination latest block is not provided"
        );

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(10000u32)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Refund),
            "Should trigger refund action when destination swap intiated and expired"
        );
    }

    #[tokio::test]
    async fn test_mapper_initiate_case() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .add_supported_chain("bitcoin_regtest".to_string())
            .build();

        let mut order = default_matched_order();

        order.source_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action when source swap without valid initiate block"
        );

        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 3;
        order.source_swap.current_confirmations = 2;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action with valid initiate block but less confirmations"
        );

        order.source_swap.current_confirmations = 3;
        order.source_swap.filled_amount = BigDecimal::from(0);

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action when source swap with valid initiate but partial filled amount"
        );

        order.source_swap.filled_amount = order.source_swap.amount.clone();
        order.create_order.additional_data.input_token_price = 0.0;
        order.create_order.additional_data.output_token_price = 0.0;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action when source swap with valid initiate block and filled amount but price drop"
        );

        order.create_order.additional_data.input_token_price = 1.0;
        order.create_order.additional_data.output_token_price = 1.0;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::Initiate),
            "Should trigger initiate action when source swap with valid initiate block and filled amount"
        );

        order.destination_swap.initiate_tx_hash = MaybeString::new("0xtx_init".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1000000u32));
        order.destination_swap.required_confirmations = 1;
        order.destination_swap.current_confirmations = 1;
        order.destination_swap.filled_amount = order.destination_swap.amount.clone();

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger initiate action when destination swap already has initiate"
        );
    }

    #[tokio::test]
    async fn test_mapper_instant_refund_case() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec![
                "arbitrum_localnet".to_string(),
                "bitcoin_regtest".to_string(),
            ])
            .build();

        let mut order = default_matched_order();

        order.create_order.additional_data.deadline = Utc::now().timestamp() - 7200;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger instant refund even when deadline is expired and there is no valid initiate"
        );

        order.source_swap.initiate_tx_hash = MaybeString::new("0xinit_tx".to_string());

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger instant refund even when deadline is expired but there is no valid initiate"
        );

        //Reset deadline
        order.create_order.additional_data.deadline = Utc::now().timestamp() + 7200;

        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 3;
        order.source_swap.current_confirmations = 3;
        order.source_swap.filled_amount = order.source_swap.amount.clone() - BigDecimal::from(1u32);

        let mapper_result = order_mapper
            .map(&order, Some(&BigDecimal::from(1000000)))
            .await
            .unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund when amount is not equal to filled amount"
        );

        order.source_swap.filled_amount = order.source_swap.amount.clone();

        //Reset deadline
        order.create_order.additional_data.deadline = Utc::now().timestamp() - 7200;

        order.destination_swap.initiate_tx_hash = MaybeString::new("0xinit_tx".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.destination_swap.required_confirmations = 3;
        order.destination_swap.current_confirmations = 3;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger instant refund even when deadline is expired but destination swap has initiate"
        );

        order.destination_swap.initiate_block_number = None;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund even when deadline is expired and there is valid initiate"
        );

        order.create_order.additional_data.deadline = Utc::now().timestamp() + 7200;
        order.create_order.additional_data.input_token_price = 0.0;
        order.create_order.additional_data.output_token_price = 0.0;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::InstantRefund),
            "Should trigger instant refund even when there is valid initiate but price drop"
        );
        // This skips initiate case
        order.source_swap.initiate_block_number = None;
        order.create_order.additional_data.input_token_price = 1.0;
        order.create_order.additional_data.output_token_price = 1.0;

        let mapper_result = order_mapper.map(&order, None).await.unwrap();
        println!("action: {:?}", mapper_result.action);
        assert!(
            matches!(mapper_result.action, HTLCAction::NoOp),
            "Should not trigger instant refund even when deadline is not expired and no price drop"
        );
    }

    #[tokio::test]
    async fn test_has_refundable_swaps() {
        let url = start_mock_server().await;

        let fiat_provider = FiatProvider::new(url, None).unwrap();

        let order_mapper = OrderMapper::builder(fiat_provider)
            .with_price_drop_threshold(PRICE_DROP_THRESHOLD)
            .with_supported_chains(vec![
                "arbitrum_localnet".to_string(),
                "bitcoin_regtest".to_string(),
            ])
            .build();

        let mut order = default_matched_order();
        let has_refundable_swaps = order_mapper.has_refundable_swaps(&vec![order.clone()]);
        println!("has_refundable_swaps: {}", has_refundable_swaps);
        assert!(matches!(has_refundable_swaps, false));

        // source swap initiate
        order.source_swap.initiate_tx_hash = MaybeString::new("0xinit_tx".to_string());
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 3;
        order.source_swap.current_confirmations = 3;

        // destination swap initiate
        order.destination_swap.initiate_tx_hash = MaybeString::new("0xinit_tx".to_string());
        order.destination_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.destination_swap.required_confirmations = 3;
        order.destination_swap.current_confirmations = 3;

        // destination swap minimum refundable time crossed
        order.destination_swap.created_at = Utc::now() - chrono::Duration::hours(2);

        order.destination_swap.redeem_tx_hash = MaybeString::new("0xredeem_tx".to_string());
        order.destination_swap.secret = MaybeString::new("0xsecret".to_string());
        let has_refundable_swaps = order_mapper.has_refundable_swaps(&vec![order.clone()]);
        println!("has_refundable_swaps: {}", has_refundable_swaps);
        assert!(
            matches!(has_refundable_swaps, false),
            "Should not trigger refundable when destination swap has redeem"
        );

        order.destination_swap.redeem_tx_hash = MaybeString::new("".to_string());
        order.destination_swap.secret = MaybeString::new("".to_string());
        order.destination_swap.refund_tx_hash = MaybeString::new("0xrefund_tx".to_string());
        order.destination_swap.refund_block_number = Some(BigDecimal::from(1u32));

        let has_refundable_swaps = order_mapper.has_refundable_swaps(&vec![order.clone()]);
        println!("has_refundable_swaps: {}", has_refundable_swaps);
        assert!(
            matches!(has_refundable_swaps, false),
            "Should not trigger refundable when destination swap has refund"
        );

        order.destination_swap.refund_tx_hash = MaybeString::new("".to_string());
        order.destination_swap.refund_block_number = None;
        let has_refundable_swaps = order_mapper.has_refundable_swaps(&vec![order.clone()]);
        println!("has_refundable_swaps: {}", has_refundable_swaps);
        assert!(
            matches!(has_refundable_swaps, true),
            "Should trigger refundable when destination swap has no redeem and refund and minimum refundable time crossed"
        );
    }
}
