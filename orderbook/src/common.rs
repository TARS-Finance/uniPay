use bigdecimal::{BigDecimal, num_bigint};
use eyre::Result;
use sha2::{Digest, Sha256};

/// Converts a decimal amount into the low/high felt representation used by Starknet hashing.
pub fn bigdecimal_to_i128s(value: &BigDecimal) -> Result<(i128, i128)> {
    let (bigint, scale) = value.as_bigint_and_exponent();

    // Normalize the scale first so the integer bytes represent the on-chain amount.
    let adjusted_bigint = if scale < 0 {
        bigint * num_bigint::BigInt::from(10).pow(-scale as u32)
    } else if scale > 0 {
        bigint / num_bigint::BigInt::from(10).pow(scale as u32)
    } else {
        bigint
    };

    let bytes = adjusted_bigint.to_bytes_le().1;
    let mut padded_bytes = vec![0_u8; 32];
    for (index, byte) in bytes.iter().enumerate().take(32) {
        padded_bytes[index] = *byte;
    }

    let low = i128::from_le_bytes(
        padded_bytes[0..16]
            .try_into()
            .map_err(|err| eyre::eyre!("failed to convert low bytes: {err}"))?,
    );
    let high = i128::from_le_bytes(
        padded_bytes[16..32]
            .try_into()
            .map_err(|err| eyre::eyre!("failed to convert high bytes: {err}"))?,
    );

    Ok((low, high))
}

/// Returns the lowercase SHA-256 digest of the concatenated byte slices.
pub fn sha256_hex(parts: &[impl AsRef<[u8]>]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_ref());
    }
    hex::encode(hasher.finalize())
}

/// Normalizes addresses so cross-chain comparisons use the same casing rules as Unipay.
/// Bitcoin and Solana addresses are case-sensitive and must not be lowercased.
pub fn normalize_address(chain: &str, address: &str) -> String {
    if chain.contains("bitcoin") || chain.contains("solana") {
        address.to_string()
    } else {
        address.to_lowercase()
    }
}

/// Splits a 32-byte hex string into eight u32 chunks for Starknet hashing.
pub fn hex_to_u32_array(hex_string: &str) -> Result<[u32; 8]> {
    let hex_str = hex_string.strip_prefix("0x").unwrap_or(hex_string);
    if hex_str.len() != 64 {
        return Err(eyre::eyre!(
            "invalid hex string length. expected 64 characters, got {}",
            hex_str.len()
        ));
    }

    let mut result = [0_u32; 8];
    for index in 0..8 {
        let start = index * 8;
        let end = start + 8;
        result[index] = u32::from_str_radix(&hex_str[start..end], 16)?;
    }

    Ok(result)
}

/// Left-pads a byte slice up to the requested width.
pub fn left_pad_bytes(slice: &[u8], len: usize) -> Vec<u8> {
    if slice.len() >= len {
        return slice.to_vec();
    }

    let mut padded = vec![0_u8; len - slice.len()];
    padded.extend_from_slice(slice);
    padded
}

/// Decodes hex input and right-pads it with zeros to a fixed size.
pub fn decode_and_pad_hex(input: &str, pad_len: usize) -> Result<Vec<u8>> {
    let bytes = hex::decode(input.trim_start_matches("0x"))?;
    if bytes.len() > pad_len {
        return Err(eyre::eyre!(
            "{input} too long: expected at most {pad_len} bytes, got {}",
            bytes.len()
        ));
    }

    let mut padded = bytes;
    padded.resize(pad_len, 0);
    Ok(padded)
}

/// Converts variable-length hex input into a fixed 32-byte array.
pub fn hex_to_hash(hex_str: &str) -> Result<[u8; 32]> {
    let clean_hex = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(clean_hex)?;

    let mut hash = [0_u8; 32];
    if bytes.len() <= 32 {
        let start_pos = 32 - bytes.len();
        hash[start_pos..].copy_from_slice(&bytes);
    } else {
        hash.copy_from_slice(&bytes[bytes.len() - 32..]);
    }

    Ok(hash)
}

/// ABI-encodes an unsigned integer as a 32-byte big-endian value.
pub fn abi_encode_uint256(value: impl Into<num_bigint::BigUint>) -> Vec<u8> {
    let bytes = value.into().to_bytes_be();
    let mut padded = vec![0_u8; 32];
    let start_pos = 32 - bytes.len();
    padded[start_pos..].copy_from_slice(&bytes);
    padded
}
