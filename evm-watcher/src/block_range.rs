//! Block range computation utilities for HTLC (Hash Time Locked Contract) events.
//!
//! This module provides functionality to compute block ranges for processing
//!  ensuring efficient events retrieval
//! while respecting maximum block span constraints.

use crate::primitives::BlockRange;

/// Groups block numbers into ranges, ensuring each range's span does not exceed `max_block_span`.
/// # Arguments
/// * `blocks` - A vector of block numbers.
/// * `max_block_span` - The maximum allowed span for each block range.
pub fn group_block_ranges(mut blocks: Vec<u64>, max_block_span: u64) -> Vec<BlockRange> {
    // remove zero blocks
    blocks.retain(|&b| b != 0);
    // Handle empty input
    if blocks.is_empty() {
        return Vec::new();
    }
    // Sort and remove duplicates in-place
    blocks.sort_unstable();
    blocks.dedup();

    // Determine the first and last block numbers
    // the blocks is guaranteed to be non-empty here due to the initial check
    let first_block = *blocks.first().unwrap_or(&0);
    let last_block = *blocks.last().unwrap_or(&0);

    // Single block case
    if blocks.len() == 1 {
        return vec![BlockRange::new(first_block, last_block)];
    }

    let mut ranges = Vec::new();
    let mut range_start = first_block;
    let mut previous_block = first_block;

    // Group blocks into ranges based on max_block_span
    for &current_block in blocks.iter().skip(1) {
        if current_block.saturating_sub(range_start) >= max_block_span {
            ranges.push(BlockRange::new(range_start, previous_block));
            range_start = current_block;
        }
        previous_block = current_block;
    }

    // Include the final range if valid
    if range_start <= last_block {
        ranges.push(BlockRange::new(range_start, last_block));
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::BlockRange;

    #[test]
    fn test_single_block() {
        let blocks = vec![100, 100];
        let result = group_block_ranges(blocks, 100);
        assert_eq!(result, vec![BlockRange::new(100, 100)]);
    }

    #[test]
    fn test_multiple_blocks_within_span() {
        let blocks = vec![100, 102, 101, 103];
        let result = group_block_ranges(blocks, 10);
        assert_eq!(result, vec![BlockRange::new(100, 103)]);
    }

    #[test]
    fn test_blocks_exceeding_span() {
        let blocks = vec![100, 102, 200, 202];
        let result = group_block_ranges(blocks, 50);
        assert_eq!(
            result,
            vec![BlockRange::new(100, 102), BlockRange::new(200, 202),]
        );
    }

    #[test]
    fn test_zero_blocks_filtered() {
        let blocks = vec![100, 200];
        let result = group_block_ranges(blocks, 50);
        assert_eq!(
            result,
            vec![BlockRange::new(100, 100), BlockRange::new(200, 200)]
        );
    }

    #[test]
    fn test_duplicate_blocks() {
        let blocks = vec![100, 100, 100, 100, 200, 200];
        let result = group_block_ranges(blocks, 50);
        assert_eq!(
            result,
            vec![BlockRange::new(100, 100), BlockRange::new(200, 200),]
        );
    }

    #[test]
    fn test_unordered_blocks() {
        let blocks = vec![300, 100, 200, 400];
        let result = group_block_ranges(blocks, 150);
        assert_eq!(
            result,
            vec![BlockRange::new(100, 200), BlockRange::new(300, 400),]
        );
    }

    #[test]
    fn test_large_block_span() {
        let blocks = vec![100, 175, 200, 250, 300, 350];
        let result = group_block_ranges(blocks, 75);
        assert_eq!(
            result,
            vec![
                BlockRange::new(100, 100),
                BlockRange::new(175, 200),
                BlockRange::new(250, 300),
                BlockRange::new(350, 350),
            ]
        );
    }
}
