use crate::{error::AppError, quote::types::QuoteAssetView, registry::Strategy};
use bigdecimal::{BigDecimal, FromPrimitive, RoundingMode, num_bigint::ToBigInt};

/// Computes an exact-in quote by converting source value into destination amount.
pub fn calculate_output_amount(
    strategy: &Strategy,
    input_amount: &BigDecimal,
    input_price: f64,
    output_price: f64,
    affiliate_fee: u64,
    slippage: u64,
) -> Result<(QuoteAssetView, QuoteAssetView), AppError> {
    validate_amount(input_amount)?;
    validate_prices(input_price, output_price)?;

    // Strategy amount limits are enforced before any fee or slippage math.
    if *input_amount < strategy.min_amount || *input_amount > strategy.max_amount {
        return Err(AppError::bad_request(format!(
            "expected amount to be within {} and {}",
            strategy.min_amount, strategy.max_amount
        )));
    }

    let input_price = BigDecimal::from_f64(input_price)
        .ok_or_else(|| AppError::bad_request("invalid input price"))?;
    let output_price = BigDecimal::from_f64(output_price)
        .ok_or_else(|| AppError::bad_request("invalid output price"))?;

    // Work in human-readable units for pricing, then convert back into on-chain integers.
    let normalized_input = divide_by_decimals(input_amount, strategy.source_asset.decimals);
    let input_usd = &normalized_input * &input_price;
    let fee_decimal = BigDecimal::from(strategy.fee + affiliate_fee) / BigDecimal::from(10_000);
    let fee_amount = &input_usd * fee_decimal;
    let net_input_usd = &input_usd - fee_amount - &strategy.fixed_fee;

    let normalized_output = net_input_usd / &output_price;
    let slippage_multiplier = BigDecimal::from(10_000 - slippage) / BigDecimal::from(10_000);
    let normalized_output = &normalized_output / slippage_multiplier;
    let output_amount = multiply_by_decimals(&normalized_output, strategy.dest_asset.decimals);
    let output_amount_int = output_amount
        .to_bigint()
        .ok_or_else(|| AppError::internal("failed to calculate output amount"))?;

    Ok((
        QuoteAssetView {
            asset: format!("{}:{}", strategy.source_chain, strategy.source_asset.asset),
            amount: input_amount.clone(),
            display: normalized_input.with_scale(8),
            value: input_usd.with_scale(4),
        },
        QuoteAssetView {
            asset: format!("{}:{}", strategy.dest_chain, strategy.dest_asset.asset),
            amount: BigDecimal::from(output_amount_int.clone()),
            display: divide_by_decimals(
                &BigDecimal::from(output_amount_int),
                strategy.dest_asset.decimals,
            )
            .with_scale(8),
            value: (normalized_output * output_price).with_scale(4),
        },
    ))
}

/// Computes an exact-out quote by solving backwards for the required source amount.
pub fn calculate_input_amount(
    strategy: &Strategy,
    output_amount: &BigDecimal,
    input_price: f64,
    output_price: f64,
    affiliate_fee: u64,
    slippage: u64,
) -> Result<(QuoteAssetView, QuoteAssetView), AppError> {
    validate_amount(output_amount)?;
    validate_prices(input_price, output_price)?;

    let input_price = BigDecimal::from_f64(input_price)
        .ok_or_else(|| AppError::bad_request("invalid input price"))?;
    let output_price = BigDecimal::from_f64(output_price)
        .ok_or_else(|| AppError::bad_request("invalid output price"))?;

    // Reverse the exact-in math: add slippage, fees, and fixed costs back onto the source side.
    let normalized_output_with_slippage =
        divide_by_decimals(output_amount, strategy.dest_asset.decimals);
    let output_usd_with_slippage = &normalized_output_with_slippage * output_price.clone();
    let slippage_multiplier = BigDecimal::from(10_000 - slippage) / BigDecimal::from(10_000);
    let normalized_output = &normalized_output_with_slippage * &slippage_multiplier;
    let output_usd = &normalized_output * output_price;
    let normalized_input = (&output_usd + &strategy.fixed_fee) / &input_price;
    let fee_multiplier =
        BigDecimal::from(10_000 - (strategy.fee + affiliate_fee)) / BigDecimal::from(10_000);
    let adjusted_input = &normalized_input / fee_multiplier;
    let input_amount = multiply_by_decimals(&adjusted_input, strategy.source_asset.decimals);
    let input_amount_int = input_amount
        .with_scale_round(0, RoundingMode::Up)
        .to_bigint()
        .ok_or_else(|| AppError::internal("failed to calculate input amount"))?;

    let input_amount_bd = BigDecimal::from(input_amount_int.clone());
    if input_amount_bd > strategy.max_amount || input_amount_bd < strategy.min_amount {
        return Err(AppError::bad_request(
            "output amount falls outside strategy range",
        ));
    }

    Ok((
        QuoteAssetView {
            asset: format!("{}:{}", strategy.source_chain, strategy.source_asset.asset),
            amount: input_amount_bd.clone(),
            display: divide_by_decimals(&input_amount_bd, strategy.source_asset.decimals)
                .with_scale(8),
            value: (adjusted_input * input_price).with_scale(4),
        },
        QuoteAssetView {
            asset: format!("{}:{}", strategy.dest_chain, strategy.dest_asset.asset),
            amount: output_amount.clone(),
            display: normalized_output_with_slippage.with_scale(8),
            value: output_usd_with_slippage.with_scale(4),
        },
    ))
}

/// Rejects zero or negative amounts up front.
fn validate_amount(amount: &BigDecimal) -> Result<(), AppError> {
    if amount <= &BigDecimal::from(0) {
        Err(AppError::bad_request("amount must be greater than 0"))
    } else {
        Ok(())
    }
}

/// Ensures quote math never runs on missing or invalid prices.
fn validate_prices(input: f64, output: f64) -> Result<(), AppError> {
    if input <= 0.0 || output <= 0.0 {
        Err(AppError::bad_request("token prices must be greater than 0"))
    } else {
        Ok(())
    }
}

/// Converts an integer on-chain amount into display units.
fn divide_by_decimals(amount: &BigDecimal, decimals: u8) -> BigDecimal {
    amount / BigDecimal::from(10_u64.pow(decimals as u32))
}

/// Converts a display-unit amount back into an integer on-chain amount.
fn multiply_by_decimals(amount: &BigDecimal, decimals: u8) -> BigDecimal {
    amount * BigDecimal::from(10_u64.pow(decimals as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Strategy, StrategyAsset};
    use tars::primitives::HTLCVersion;

    fn strategy() -> Strategy {
        Strategy {
            id: "btc_eth".to_string(),
            source_chain_address: "maker-src".to_string(),
            dest_chain_address: "maker-dst".to_string(),
            source_chain: "bitcoin_testnet".to_string(),
            dest_chain: "ethereum_sepolia".to_string(),
            source_asset: StrategyAsset {
                asset: "primary".to_string(),
                htlc_address: "primary".to_string(),
                token_address: "primary".to_string(),
                token_id: "bitcoin".to_string(),
                decimals: 8,
                version: HTLCVersion::V2,
            },
            dest_asset: StrategyAsset {
                asset: "0xhtlc".to_string(),
                htlc_address: "0xhtlc".to_string(),
                token_address: "0xtoken".to_string(),
                token_id: "ethereum".to_string(),
                decimals: 18,
                version: HTLCVersion::V2,
            },
            makers: vec![],
            min_amount: BigDecimal::from(1000),
            max_amount: BigDecimal::from(100_000_000_u64),
            min_source_timelock: 12,
            destination_timelock: 6,
            min_source_confirmations: 1,
            fee: 30,
            fixed_fee: BigDecimal::from(0),
            max_slippage: 300,
        }
    }

    #[test]
    fn exact_in_quote_produces_positive_destination_amount() {
        let (_, destination) = calculate_output_amount(
            &strategy(),
            &BigDecimal::from(1_000_000_u64),
            100_000.0,
            2_500.0,
            0,
            50,
        )
        .unwrap();

        assert!(destination.amount > BigDecimal::from(0));
        assert!(destination.value > BigDecimal::from(0));
    }
}
