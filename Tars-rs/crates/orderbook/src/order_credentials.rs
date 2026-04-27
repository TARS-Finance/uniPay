use crate::primitives::MatchedOrderVerbose;
use api::primitives::{Response, Status};
use eyre::{eyre, Result};
use reqwest::{Client, RequestBuilder, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, fmt::Debug, marker::PhantomData, time::Duration};

/// The timeout for requests to the order credentials provider.
const REQUEST_TIMEOUT_SECS: Duration = Duration::from_secs(10);

/// A struct that contains an order and its credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderWithCredentials {
    /// The order.
    pub order: MatchedOrderVerbose,

    /// The credentials.
    pub credentials: Credentials,
}

/// A struct that contains the credentials for an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    /// The secret.
    pub secret: String,

    /// The private key.
    pub private_key: String,
}

/// Response payload for credential generation
#[derive(Debug, Serialize, Deserialize)]
pub struct GenerateCredentialsResponse {
    /// Hash of the generated secret for future retrieval
    pub secret_hash: String,

    /// X-only public key derived from the generated credentials
    pub x_only_pubkey: String,
}

/// Marker types for capabilities
pub struct RetrieveEnabled;
pub struct GenerateEnabled;

pub struct RetrieveDisabled;
pub struct GenerateDisabled;

/// Builder for `OrderCredentialsProvider`.
///
/// The builder uses two type parameters, `R` and `G`, to track (at the type level)
/// whether the retrieve and generate URLs have been set. This allows us to provide
/// compile-time guarantees about which operations are available on the provider.
/// The `_capability` field is a `PhantomData<Capabilities<R, G>>`, which ensures
/// that the type parameters are preserved and used by the Rust type system,
/// even though they are not stored as runtime data.
#[derive(Debug)]
pub struct OrderCredentialsProviderBuilder<R = RetrieveDisabled, G = GenerateDisabled> {
    /// The URL to retrieve credentials from.
    retrieve_url: Option<Url>,

    /// The URL to generate credentials from.
    generate_url: Option<Url>,

    /// Phantom data to encode capabilities at the type level.
    /// This is necessary because the builder can be in different states
    /// depending on which URLs have been set, and we want to enforce
    /// correct usage at compile time.
    _capability: PhantomData<(R, G)>,
}

impl OrderCredentialsProviderBuilder<RetrieveDisabled, GenerateDisabled> {
    /// Creates a new builder with both retrieve and generate URLs disabled.
    pub fn new() -> Self {
        Self {
            retrieve_url: None,
            generate_url: None,
            _capability: PhantomData,
        }
    }
}

impl<R, G> OrderCredentialsProviderBuilder<R, G> {
    /// Sets the retrieve URL for the builder.
    ///
    /// # Arguments
    ///
    /// * `url` - The URL to retrieve credentials from.
    ///
    /// # Returns
    ///
    /// A new builder with the retrieve URL set.
    pub fn with_retrieve_url(
        self,
        url: Url,
    ) -> OrderCredentialsProviderBuilder<RetrieveEnabled, G> {
        OrderCredentialsProviderBuilder {
            retrieve_url: Some(url),
            generate_url: self.generate_url,
            _capability: PhantomData,
        }
    }

    /// Sets the generate URL for the builder.
    ///
    /// # Arguments
    ///
    /// * `url` - The URL to generate credentials from.
    ///
    /// # Returns
    ///
    /// A new builder with the generate URL set.
    pub fn with_generate_url(
        self,
        url: Url,
    ) -> OrderCredentialsProviderBuilder<R, GenerateEnabled> {
        OrderCredentialsProviderBuilder {
            retrieve_url: self.retrieve_url,
            generate_url: Some(url),
            _capability: PhantomData,
        }
    }

    /// Builds the `OrderCredentialsProvider` from the builder.
    ///
    /// # Returns
    ///
    /// A new `OrderCredentialsProvider` with the configured URLs and client.
    pub fn build(self) -> OrderCredentialsProvider<R, G> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT_SECS)
            .build()
            .expect("Failed to create client");

        OrderCredentialsProvider {
            retrieve_url: self.retrieve_url,
            generate_url: self.generate_url,
            client,
            _capability: PhantomData,
        }
    }
}

/// A provider of order credentials.
///
/// This is used to get the credentials for an order.
#[derive(Debug, Clone)]
pub struct OrderCredentialsProvider<R = RetrieveEnabled, G = GenerateEnabled> {
    /// The URL to retrieve credentials from.
    retrieve_url: Option<Url>,

    /// The URL to generate credentials from.
    generate_url: Option<Url>,

    /// The client.
    client: Client,

    /// Phantom data to encode capabilities at the type level.
    /// This is necessary because the builder can be in different states
    /// depending on which URLs have been set, and we want to enforce
    /// correct usage at compile time.
    _capability: PhantomData<(R, G)>,
}

impl OrderCredentialsProvider<RetrieveDisabled, GenerateDisabled> {
    /// Creates a new builder with both retrieve and generate URLs disabled.
    pub fn builder() -> OrderCredentialsProviderBuilder<RetrieveDisabled, GenerateDisabled> {
        OrderCredentialsProviderBuilder::new()
    }
}

impl<R, G> OrderCredentialsProvider<R, G> {
    /// Sends a request to the relay and returns the result
    ///
    /// # Arguments
    ///
    /// * `request` - The request to send
    ///
    /// # Returns
    ///
    /// The result of the request
    async fn send_request<T: DeserializeOwned + Debug>(
        &self,
        request: RequestBuilder,
    ) -> Result<Option<T>> {
        let response = request
            .send()
            .await
            .map_err(|e| eyre!("Failed to send request: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_message = response
                .text()
                .await
                .map_err(|e| eyre!("Failed to get error message: {}", e))?;

            return Err(eyre!(
                "Request failed with error: {} and status: {}",
                error_message,
                status
            ));
        }

        let result: Response<T> = response
            .json()
            .await
            .map_err(|e| eyre!("Failed to parse response: {}", e))?;

        match result.status {
            Status::Ok => Ok(result.result),
            Status::Error => Err(eyre!(
                "Request failed: {}",
                result.error.unwrap_or("Unknown error".to_string())
            )),
        }
    }
}

impl<G> OrderCredentialsProvider<RetrieveEnabled, G> {
    /// Retrieves the credentials for a list of orders.
    ///
    /// # Arguments
    ///
    /// * `pending_orders` - The list of orders to retrieve credentials for.
    ///
    /// # Returns
    ///
    /// A tuple containing the list of orders with credentials and the list of secret hashes that were not found.
    pub async fn retrieve_credentials(
        &self,
        pending_orders: Vec<MatchedOrderVerbose>,
    ) -> Result<(Vec<OrderWithCredentials>, Vec<String>)> {
        if pending_orders.is_empty() {
            return Ok((vec![], vec![]));
        }

        let secret_hashes = pending_orders
            .iter()
            .map(|o| o.create_order.secret_hash.clone())
            .collect::<Vec<String>>();

        let body = json!({
            "action": "retrieve",
            "secret_hashes": secret_hashes
        });

        let retrieve_url = self
            .retrieve_url
            .as_ref()
            .cloned()
            .ok_or_else(|| eyre!("Retrieve URL not set"))?;

        let request = self.client.post(retrieve_url).json(&body);

        let credentials_map: HashMap<String, Credentials> = self
            .send_request(request)
            .await?
            .ok_or_else(|| eyre!("Failed to retrieve credentials"))?;

        let mut processable_orders = Vec::new();
        let mut non_processable_secret_hashes = Vec::new();

        for order in pending_orders {
            let secret_hash = order.create_order.secret_hash.clone();
            if let Some(credentials) = credentials_map.get(&secret_hash) {
                processable_orders.push(OrderWithCredentials {
                    order,
                    credentials: credentials.clone(),
                });
            } else {
                non_processable_secret_hashes.push(secret_hash);
            }
        }

        Ok((processable_orders, non_processable_secret_hashes))
    }

    /// Updates the credentials for an order.
    ///
    /// # Arguments
    ///
    /// * `secret` - The secret to update.
    ///
    /// # Returns
    ///
    /// A result indicating whether the credentials were updated successfully.
    pub async fn update_credentials(&self, secret: &str) -> Result<()> {
        let body = json!({
            "action" : "update",
            "secret" : secret,
        });

        let retrieve_url = self
            .retrieve_url
            .as_ref()
            .cloned()
            .ok_or_else(|| eyre!("Retrieve URL not set"))?;

        let request = self.client.post(retrieve_url).json(&body);

        let _: Option<String> = self.send_request(request).await?;

        Ok(())
    }
}

impl<R> OrderCredentialsProvider<R, GenerateEnabled> {
    /// Generates credentials for an order.
    ///
    /// # Arguments
    ///
    /// * `secret_hash` - The secret hash to generate credentials for.
    ///
    /// # Returns
    ///
    /// A result containing the generated credentials.
    pub async fn generate_credentials(
        &self,
        secret_hash: Option<String>,
    ) -> Result<GenerateCredentialsResponse> {
        let body = json!({
            "action" : "generate",
            "secret_hash" : secret_hash,
        });

        let generate_url = self
            .generate_url
            .as_ref()
            .cloned()
            .ok_or_else(|| eyre!("Generate URL not set"))?;

        let request = self.client.post(generate_url).json(&body);

        let response: GenerateCredentialsResponse = self
            .send_request(request)
            .await?
            .ok_or_else(|| eyre!("Failed to generate credentials"))?;

        Ok(response)
    }
}

/// A provider that retrieves credentials.
pub type RetrieveOrderCredentialsProvider =
    OrderCredentialsProvider<RetrieveEnabled, GenerateDisabled>;

/// A provider that generates credentials.
pub type GenerateOrderCredentialsProvider =
    OrderCredentialsProvider<RetrieveDisabled, GenerateEnabled>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{AdditionalData, CreateOrder, MaybeString, SingleSwap};
    use bigdecimal::BigDecimal;
    use chrono::Utc;
    use primitives::HTLCVersion;
    use std::str::FromStr;
    use utils::gen_secret;

    const GENERATE_URL: &str =
        "http://localhost:4575/credentials/c4b8f7e2a1d9e345f06b9a1c3d7f8e2b0a5d6c1e9b3f7a2d4c6e8b0f3a2d7e1c";
    const RETRIEVE_URL: &str =
        "http://localhost:4575/credentials/9e3b7a5d0c1f6b2d8a4e3f1c7d9b6a0e2c5d3a9f0e7b4c6d1a3e9b7c2f5d0a1";

    #[tokio::test]
    async fn test_generate_credentials() -> Result<()> {
        let generate_url = Url::parse(GENERATE_URL).unwrap();

        let provider: OrderCredentialsProvider<RetrieveDisabled> =
            OrderCredentialsProvider::builder()
                .with_generate_url(generate_url)
                .build();

        // Case 1 : Generate credentials without secret hash
        let response = provider.generate_credentials(None).await;
        dbg!(&response);
        assert!(response.is_ok());

        let response = response.unwrap();
        dbg!(&response);

        // Case 2 : Generate credentials with secret hash
        let (_, secret_hash) = gen_secret();

        let response = provider
            .generate_credentials(Some(secret_hash.to_string()))
            .await;

        assert!(response.is_ok());

        let response = response.unwrap();

        assert_eq!(response.secret_hash, secret_hash.to_string());
        dbg!(&response);

        Ok(())
    }

    // Helper function to create a sample MatchedOrderVerbose
    fn create_sample_order(secret_hash: &str, create_id: &str) -> MatchedOrderVerbose {
        let now = Utc::now();

        MatchedOrderVerbose {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            source_swap: SingleSwap {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                swap_id: "test_swap_id".to_string(),
                chain: "arbitrum_localnet".to_string(),
                asset: "0x0165878A594ca255338adfa4d48449f69242Eb8F".to_string(),
                htlc_address: None,
                token_address: None,
                initiator: "0x0073C6DB661a35c3465734fCBBaccdCb1613b2eB".to_string(),
                redeemer: "0x70997970c51812dc3a010c7d01b50e0d17dc79c8".to_string(),
                timelock: 1200,
                filled_amount: BigDecimal::from_str("10000").unwrap(),
                amount: BigDecimal::from_str("10000").unwrap(),
                secret_hash: secret_hash.to_string(),
                secret: MaybeString::new("test_secret".to_string()),
                initiate_tx_hash: MaybeString::new("0x123".to_string()),
                redeem_tx_hash: MaybeString::new("0x456".to_string()),
                refund_tx_hash: MaybeString::new("".to_string()),
                initiate_block_number: Some(BigDecimal::from_str("43").unwrap()),
                redeem_block_number: Some(BigDecimal::from_str("44").unwrap()),
                refund_block_number: None,
                required_confirmations: 1,
                current_confirmations: 1,
                initiate_timestamp: None,
                redeem_timestamp: None,
                refund_timestamp: None,
            },
            destination_swap: SingleSwap {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                swap_id: "dest_swap_id".to_string(),
                chain: "bitcoin_regtest".to_string(),
                asset: "primary".to_string(),
                token_address: None,
                htlc_address: None,
                initiator: "test_initiator".to_string(),
                redeemer: "test_redeemer".to_string(),
                timelock: 12,
                filled_amount: BigDecimal::from_str("1000").unwrap(),
                amount: BigDecimal::from_str("1000").unwrap(),
                secret_hash: secret_hash.to_string(),
                secret: MaybeString::new("test_secret".to_string()),
                initiate_tx_hash: MaybeString::new("0x123".to_string()),
                redeem_tx_hash: MaybeString::new("0x456".to_string()),
                refund_tx_hash: MaybeString::new("".to_string()),
                initiate_block_number: Some(BigDecimal::from_str("107").unwrap()),
                redeem_block_number: Some(BigDecimal::from_str("107").unwrap()),
                refund_block_number: None,
                required_confirmations: 1,
                current_confirmations: 1,
                initiate_timestamp: None,
                redeem_timestamp: None,
                refund_timestamp: None,
            },
            create_order: CreateOrder {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                create_id: create_id.to_string(),
                block_number: BigDecimal::from_str("42").unwrap(),
                source_chain: "arbitrum_localnet".to_string(),
                destination_chain: "bitcoin_regtest".to_string(),
                source_asset: "0x0165878A594ca255338adfa4d48449f69242Eb8F".to_string(),
                destination_asset: "primary".to_string(),
                initiator_source_address: "0x0073C6DB661a35c3465734fCBBaccdCb1613b2eB".to_string(),
                initiator_destination_address: "test_dest_address".to_string(),
                source_amount: BigDecimal::from_str("10000").unwrap(),
                destination_amount: BigDecimal::from_str("1000").unwrap(),
                fee: BigDecimal::from_str("0.000090000000000000001880").unwrap(),
                nonce: BigDecimal::from_str("1").unwrap(),
                min_destination_confirmations: 1,
                timelock: 1200,
                secret_hash: secret_hash.to_string(),
                user_id: Some("test_user".to_string()),
                affiliate_fees: None,
                additional_data: AdditionalData {
                    source_delegator: None,
                    strategy_id: "arbrry".to_string(),
                    bitcoin_optional_recipient: Some("test_recipient".to_string()),
                    input_token_price: 1.0,
                    output_token_price: 1.0,
                    sig: "test_signature".to_string(),
                    deadline: 1750746826,
                    instant_refund_tx_bytes: None,
                    redeem_tx_bytes: None,
                    tx_hash: Some(
                        "0xe73a28927c42b9dcbe711214da176073e4e08ff8bb58cc0c0fb937738412e0bf"
                            .to_string(),
                    ),
                    is_blacklisted: false,
                    integrator: None,
                    version: HTLCVersion::V1,
                    bitcoin: None,
                },
            },
        }
    }

    #[tokio::test]
    async fn test_retrieve_credentials() -> Result<()> {
        let retrieve_url = Url::parse(RETRIEVE_URL).unwrap();
        let generate_url = Url::parse(GENERATE_URL).unwrap();

        let provider: OrderCredentialsProvider = OrderCredentialsProvider::builder()
            .with_retrieve_url(retrieve_url)
            .with_generate_url(generate_url)
            .build();

        // Generate secrets first.
        let (_, secret_hash) = gen_secret();
        let response = provider
            .generate_credentials(Some(secret_hash.to_string()))
            .await;

        assert!(response.is_ok());

        let order = create_sample_order(&secret_hash.to_string(), "test_create_id");

        let credentials = provider.retrieve_credentials(vec![order]).await;

        assert!(credentials.is_ok());

        let (orders, non_processable_secret_hashes) = credentials.unwrap();

        assert_eq!(orders.len(), 1);
        assert_eq!(non_processable_secret_hashes.len(), 0);
        dbg!(&orders.first().unwrap().credentials);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_credentials() -> Result<()> {
        let retrieve_url = Url::parse(RETRIEVE_URL).unwrap();
        let generate_url = Url::parse(GENERATE_URL).unwrap();

        let provider = OrderCredentialsProvider::builder()
            .with_retrieve_url(retrieve_url)
            .with_generate_url(generate_url)
            .build();

        // Generate credentials first.
        let (secret, secret_hash) = gen_secret();
        let secret_hash = secret_hash.to_string().trim_start_matches("0x").to_string();

        let response = provider
            .generate_credentials(Some(secret_hash.to_string()))
            .await;
        assert!(response.is_ok());

        // Update credentials.
        let credentials = provider
            .update_credentials(&secret.to_string().trim_start_matches("0x"))
            .await;

        assert!(credentials.is_ok());

        // Retrieve credentials.
        let order = create_sample_order(&secret_hash.to_string(), "test_create_id");

        let credentials = provider.retrieve_credentials(vec![order]).await;

        assert!(credentials.is_ok());

        let (orders, non_processable_secret_hashes) = credentials.unwrap();

        assert_eq!(orders.len(), 1);
        assert_eq!(non_processable_secret_hashes.len(), 0);
        let credentials = orders.first().unwrap().credentials.clone();
        dbg!(&credentials);

        assert_eq!(
            credentials.secret,
            secret.to_string().trim_start_matches("0x")
        );

        Ok(())
    }
}
