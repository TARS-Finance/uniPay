use crate::{
    config::settings::MarketDataSettings,
    pricing::types::{MidPriceSnapshot, PriceLevel},
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub struct VwmpSample {
    pub mid: Decimal,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub bid_depth_usd: Decimal,
    pub ask_depth_usd: Decimal,
    pub weight: Decimal,
    pub is_cex: bool,
}

/// Computes the snapshot fields from the retained VWMP samples.
pub fn snapshot_from_samples(
    samples: &[VwmpSample],
    min_active_venues: u8,
    computed_at: DateTime<Utc>,
) -> Option<MidPriceSnapshot> {
    if samples.is_empty() {
        return None;
    }

    let (weighted_sum, weight_sum) = samples.iter().fold(
        (Decimal::ZERO, Decimal::ZERO),
        |(weighted_acc, weight_acc), sample| {
            (
                weighted_acc + sample.mid * sample.weight,
                weight_acc + sample.weight,
            )
        },
    );
    if weight_sum <= Decimal::ZERO {
        return None;
    }

    let cex_samples = samples
        .iter()
        .filter(|sample| sample.is_cex)
        .collect::<Vec<_>>();
    let aggregator_only = cex_samples.is_empty();

    Some(MidPriceSnapshot {
        vwmp: weighted_sum / weight_sum,
        best_bid: cex_samples
            .iter()
            .filter_map(|sample| (sample.best_bid > Decimal::ZERO).then_some(sample.best_bid))
            .max()
            .unwrap_or(Decimal::ZERO),
        best_ask: cex_samples
            .iter()
            .filter_map(|sample| (sample.best_ask > Decimal::ZERO).then_some(sample.best_ask))
            .min()
            .unwrap_or(Decimal::ZERO),
        bid_depth_usd: cex_samples
            .iter()
            .fold(Decimal::ZERO, |acc, sample| acc + sample.bid_depth_usd),
        ask_depth_usd: cex_samples
            .iter()
            .fold(Decimal::ZERO, |acc, sample| acc + sample.ask_depth_usd),
        active_venue_count: u8::try_from(samples.len()).unwrap_or(u8::MAX),
        computed_at,
        staleness_ok: samples.len() >= usize::from(min_active_venues),
        aggregator_only,
    })
}

/// Returns true when a sample lies within the configured median-relative threshold.
pub fn within_outlier_threshold(mid: Decimal, median: Decimal, threshold_pct: Decimal) -> bool {
    if median == Decimal::ZERO {
        return mid == Decimal::ZERO;
    }

    let threshold = threshold_pct / Decimal::new(100, 0);
    let deviation = (mid - median).abs() / median.abs();
    deviation <= threshold
}

/// Computes USD depth over the first `depth` levels.
pub fn depth_usd(levels: &[PriceLevel], depth: usize) -> Decimal {
    levels.iter().take(depth).fold(Decimal::ZERO, |acc, level| {
        acc + (level.price * level.quantity)
    })
}

/// Computes realized volatility from the retained VWMP history.
pub fn compute_realized_volatility(
    history: &VecDeque<(DateTime<Utc>, Decimal)>,
    settings: &MarketDataSettings,
) -> Option<Decimal> {
    if history.len() < 2 {
        return None;
    }

    let mut returns = Vec::with_capacity(history.len() - 1);
    let prices = history.iter().map(|(_, price)| *price).collect::<Vec<_>>();
    for index in 1..prices.len() {
        let prev = prices[index - 1];
        if prev <= Decimal::ZERO {
            continue;
        }
        returns.push((prices[index] - prev) / prev);
    }

    if returns.is_empty() {
        return None;
    }

    let mean = returns.iter().sum::<Decimal>() / Decimal::try_from(returns.len()).ok()?;
    let sum_sq = returns
        .iter()
        .map(|ret| (ret - mean) * (ret - mean))
        .sum::<Decimal>();
    if sum_sq == Decimal::ZERO {
        return Some(Decimal::ZERO);
    }

    let variance = sum_sq / Decimal::try_from(returns.len().saturating_sub(1)).ok()?;
    let stddev = variance.sqrt().unwrap_or(Decimal::ZERO);
    if stddev <= Decimal::ZERO {
        return Some(Decimal::ZERO);
    }

    let interval_secs = settings.vol_sample_interval_secs.max(1) as f64;
    let periods_per_year = (365.0 * 24.0 * 60.0 * 60.0) / interval_secs;
    let annualization = Decimal::try_from(periods_per_year.sqrt()).ok()?;
    Some(stddev * annualization)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn snapshot_marks_aggregator_only_when_no_cex_sample_exists() {
        let snapshot = snapshot_from_samples(
            &[VwmpSample {
                mid: Decimal::new(100, 0),
                best_bid: Decimal::ZERO,
                best_ask: Decimal::ZERO,
                bid_depth_usd: Decimal::ZERO,
                ask_depth_usd: Decimal::ZERO,
                weight: Decimal::ONE,
                is_cex: false,
            }],
            1,
            Utc::now(),
        )
        .unwrap();

        assert!(snapshot.aggregator_only);
        assert_eq!(snapshot.vwmp, Decimal::new(100, 0));
        assert_eq!(snapshot.best_bid, Decimal::ZERO);
        assert_eq!(snapshot.best_ask, Decimal::ZERO);
    }

    #[test]
    fn outlier_threshold_rejects_far_sample() {
        assert!(within_outlier_threshold(
            Decimal::new(102, 0),
            Decimal::new(100, 0),
            Decimal::new(5, 0),
        ));
        assert!(!within_outlier_threshold(
            Decimal::new(120, 0),
            Decimal::new(100, 0),
            Decimal::new(5, 0),
        ));
    }
}
