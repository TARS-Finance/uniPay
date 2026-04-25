use crate::common::{
    abi_encode_uint256, bigdecimal_to_i128s, decode_and_pad_hex, hex_to_hash, hex_to_u32_array,
    left_pad_bytes,
};
use bigdecimal::{BigDecimal, ToPrimitive, Zero, num_bigint::BigUint};
use bitcoin::XOnlyPublicKey;
use sha2::{Digest, Sha256};
use starknet::core::types::Felt;
use starknet_crypto::poseidon_hash_many;
use std::{collections::HashMap, str::FromStr};
use tars::{
    bitcoin::{HTLCParams, get_bitcoin_network, get_htlc_address},
    orderbook::primitives::{AdditionalData, Order, SignableAdditionalData},
    primitives::HTLCVersion,
};

/// Generates deterministic swap IDs using the chain-specific hashing scheme.
#[derive(Debug, Clone)]
pub struct SwapIdGenerator {
    chain_ids: HashMap<String, u128>,
}

impl SwapIdGenerator {
    /// Creates the swap ID generator from configured chain IDs.
    pub fn new(chain_ids: HashMap<String, u128>) -> Self {
        Self { chain_ids }
    }

    /// Dispatches swap ID generation to the matching chain-specific algorithm.
    pub fn generate_swap_id(
        &self,
        chain: &str,
        initiator_address: &str,
        redeemer_address: &str,
        timelock: u64,
        secret_hash: &str,
        amount: &BigDecimal,
        htlc_version: &HTLCVersion,
        htlc_address: &str,
    ) -> eyre::Result<String> {
        let chain_id = self
            .chain_ids
            .get(chain)
            .ok_or_else(|| eyre::eyre!("missing chain_id for chain: {chain}"))?;

        let params = SwapParams {
            chain_id: chain_id.to_string(),
            initiator_address: initiator_address.to_string(),
            redeemer_address: redeemer_address.to_string(),
            timelock,
            secret_hash: secret_hash.to_string(),
            amount: amount.clone(),
            htlc_version: htlc_version.clone(),
            htlc_address: htlc_address.to_string(),
        };

        // Each chain family reproduces the identifier scheme expected by its HTLC contracts.
        if chain.contains("bitcoin") || chain.contains("alpen") || chain.contains("litecoin") {
            self.generate_bitcoin_swap_id(chain, &params)
        } else if chain.contains("solana") {
            self.generate_solana_swap_id(&params)
        } else if chain.contains("starknet") {
            self.generate_starknet_swap_id(&params)
        } else if chain.contains("sui") {
            self.generate_sui_swap_id(&params)
        } else {
            self.generate_evm_swap_id(&params)
        }
    }

    /// Rebuilds the Solana swap hash from chain ID, secret hash, and initiator bytes.
    fn generate_solana_swap_id(&self, params: &SwapParams) -> eyre::Result<String> {
        let chain_id_num: u64 = params.chain_id.parse()?;
        let chain_id_big = BigUint::from(chain_id_num);
        let chain_id_padded = left_pad_bytes(&chain_id_big.to_bytes_be(), 32);
        let secret_hash_padded = decode_and_pad_hex(&params.secret_hash, 32)?;
        let initiator_bytes = bs58::decode(params.initiator_address.trim_start_matches("0x"))
            .into_vec()
            .map_err(|_| {
                eyre::eyre!(
                    "invalid base58 initiator address for solana: {}",
                    params.initiator_address
                )
            })?;

        let mut data = Vec::new();
        data.extend(chain_id_padded);
        data.extend(secret_hash_padded);
        data.extend(initiator_bytes);

        Ok(hex::encode(Sha256::digest(&data)))
    }

    /// Reconstructs the Bitcoin HTLC address used as the swap identifier.
    fn generate_bitcoin_swap_id(&self, chain: &str, params: &SwapParams) -> eyre::Result<String> {
        let network = get_bitcoin_network(chain)?;
        let initiator_pubkey = XOnlyPublicKey::from_str(&params.initiator_address)?;
        let redeemer_pubkey = XOnlyPublicKey::from_str(&params.redeemer_address)?;
        let amount: u64 = params.amount.to_string().parse().map_err(|_| {
            eyre::eyre!(
                "bitcoin amount is too large, negative, or not a whole number: {}",
                params.amount
            )
        })?;

        let secret_hash_bytes = hex::decode(params.secret_hash.trim_start_matches("0x"))?;
        if secret_hash_bytes.len() != 32 {
            return Err(eyre::eyre!(
                "bitcoin secret hash must be 32 bytes, got {} bytes",
                secret_hash_bytes.len()
            ));
        }

        let mut secret_hash_array = [0_u8; 32];
        secret_hash_array.copy_from_slice(&secret_hash_bytes[..32]);

        let htlc_params = HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount,
            secret_hash: secret_hash_array,
            timelock: params.timelock,
        };

        Ok(get_htlc_address(&htlc_params, network)?.to_string())
    }

    /// Rebuilds the Starknet Poseidon hash used for swap identifiers.
    fn generate_starknet_swap_id(&self, params: &SwapParams) -> eyre::Result<String> {
        let chain_id_felt = Felt::from_dec_str(&params.chain_id)?;
        let initiator_address_felt = Felt::from_hex(&params.initiator_address)?;
        let redeemer_address_felt = Felt::from_hex(&params.redeemer_address)?;
        let timelock_felt = Felt::from_dec_str(&params.timelock.to_string())?;
        let secret_hash_array = hex_to_u32_array(&params.secret_hash)?;
        let (amount_low, amount_high) = bigdecimal_to_i128s(&params.amount)?;
        let amount_low_felt = Felt::from_dec_str(&amount_low.to_string())?;
        let amount_high_felt = Felt::from_dec_str(&amount_high.to_string())?;

        let mut data = vec![
            chain_id_felt,
            initiator_address_felt,
            redeemer_address_felt,
            timelock_felt,
            amount_low_felt,
            amount_high_felt,
        ];
        for part in secret_hash_array {
            data.push(Felt::from(part));
        }

        Ok(poseidon_hash_many(&data)
            .to_hex_string()
            .trim_start_matches("0x")
            .to_string())
    }

    /// Rebuilds the EVM swap hash, including HTLC version-specific fields.
    fn generate_evm_swap_id(&self, params: &SwapParams) -> eyre::Result<String> {
        let chain_id_num: u64 = params.chain_id.parse()?;
        let chain_id_padded = left_pad_bytes(&BigUint::from(chain_id_num).to_bytes_be(), 32);
        let secret_hash_padded = decode_and_pad_hex(&params.secret_hash, 32)?;
        let initiator_bytes = hex_to_hash(&params.initiator_address)?;

        let mut data = Vec::new();
        data.extend(chain_id_padded);
        data.extend(secret_hash_padded);
        data.extend(initiator_bytes);

        if params.htlc_version == HTLCVersion::V2 || params.htlc_version == HTLCVersion::V3 {
            let redeemer_bytes = hex_to_hash(&params.redeemer_address)?;
            let timelock_bytes = abi_encode_uint256(params.timelock);
            let amount_big = BigUint::from_str(&params.amount.to_string())
                .map_err(|_| eyre::eyre!("invalid amount: {}", params.amount))?;
            let amount_bytes = abi_encode_uint256(amount_big);

            data.extend(redeemer_bytes);
            data.extend(timelock_bytes);
            data.extend(amount_bytes);
        }

        if params.htlc_version == HTLCVersion::V3 {
            let htlc_address_bytes = hex_to_hash(&params.htlc_address)?;
            data.extend(htlc_address_bytes);
        }

        Ok(hex::encode(Sha256::digest(&data)))
    }

    /// Rebuilds the Sui swap hash from the Move-compatible byte layout.
    fn generate_sui_swap_id(&self, params: &SwapParams) -> eyre::Result<String> {
        let chain_id_value: u8 = params.chain_id.parse()?;
        let mut chain_id = vec![0_u8; 32];
        chain_id[31] = chain_id_value;
        let secret_hash = hex::decode(&params.secret_hash)?;
        let initiator_bytes = hex::decode(params.initiator_address.trim_start_matches("0x"))?;
        let redeemer_bytes = hex::decode(params.redeemer_address.trim_start_matches("0x"))?;
        let mut timelock_bytes = vec![0_u8; 32];
        timelock_bytes[..8].copy_from_slice(&params.timelock.to_le_bytes());
        let amount_i64 = params
            .amount
            .to_i64()
            .ok_or_else(|| eyre::eyre!("failed to convert amount to i64"))?;
        let amount_bytes = amount_i64.to_le_bytes();
        let reg_id_bytes = hex::decode(params.htlc_address.trim_start_matches("0x"))?;

        let mut data = Vec::new();
        data.extend_from_slice(&chain_id);
        data.extend_from_slice(&secret_hash);
        data.extend_from_slice(&initiator_bytes);
        data.extend_from_slice(&redeemer_bytes);
        data.extend_from_slice(&timelock_bytes);
        data.extend_from_slice(&amount_bytes);
        data.extend_from_slice(&reg_id_bytes);

        Ok(hex::encode(Sha256::digest(&data)))
    }
}

/// Normalized swap ID inputs shared across the chain-specific generators.
#[derive(Debug, Clone)]
struct SwapParams {
    chain_id: String,
    initiator_address: String,
    redeemer_address: String,
    timelock: u64,
    secret_hash: String,
    amount: BigDecimal,
    htlc_version: HTLCVersion,
    htlc_address: String,
}

/// Computes the value spread between source and destination legs in USD.
pub fn calculate_match_fee(
    source_amount: &BigDecimal,
    destination_amount: &BigDecimal,
    source_decimals: u8,
    destination_decimals: u8,
    input_token_price: f64,
    output_token_price: f64,
) -> eyre::Result<BigDecimal> {
    if source_amount.is_zero() || destination_amount.is_zero() {
        return Ok(BigDecimal::zero());
    }

    let input_price = BigDecimal::from_str(&input_token_price.to_string())?;
    let output_price = BigDecimal::from_str(&output_token_price.to_string())?;
    let source_scale = BigDecimal::from(10_u64.pow(source_decimals as u32));
    let destination_scale = BigDecimal::from(10_u64.pow(destination_decimals as u32));

    let source_usd = (source_amount / source_scale) * input_price;
    let destination_usd = (destination_amount / destination_scale) * output_price;
    Ok(source_usd - destination_usd)
}

/// Signs the normalized order payload when a quote private key is configured.
pub async fn sign_order_payload(
    order: &Order<AdditionalData>,
    private_key_hex: Option<&str>,
) -> eyre::Result<String> {
    use alloy::signers::{Signer, local::PrivateKeySigner};

    let Some(private_key_hex) = private_key_hex else {
        return Ok(String::new());
    };

    let private_key = hex::decode(private_key_hex)?;
    let signer = PrivateKeySigner::from_slice(&private_key)?;
    let signable_order: Order<SignableAdditionalData> = order.clone().into();
    let json = serde_json::to_string(&signable_order)?;
    let signature = signer.sign_message(json.as_bytes()).await?;
    Ok(hex::encode(signature.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evm_swap_id_is_deterministic() {
        let generator = SwapIdGenerator::new(HashMap::from([(
            "ethereum_sepolia".to_string(),
            11155111_u128,
        )]));
        let amount = BigDecimal::from(1000);
        let first = generator
            .generate_swap_id(
                "ethereum_sepolia",
                "0x1111111111111111111111111111111111111111",
                "0x2222222222222222222222222222222222222222",
                100,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &amount,
                &HTLCVersion::V2,
                "0x3333333333333333333333333333333333333333",
            )
            .unwrap();
        let second = generator
            .generate_swap_id(
                "ethereum_sepolia",
                "0x1111111111111111111111111111111111111111",
                "0x2222222222222222222222222222222222222222",
                100,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &amount,
                &HTLCVersion::V2,
                "0x3333333333333333333333333333333333333333",
            )
            .unwrap();
        assert_eq!(first, second);
    }
}
