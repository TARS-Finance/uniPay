//! Validation utilities for Bitcoin HTLC primitives.
//!
//! Adapted from garden-rs `crates/bitcoin/src/htlc/validate.rs`.

use sha2::{Digest, Sha256};

/// Validates that the provided secret, when SHA256-hashed, matches the
/// expected hash.
///
/// # Arguments
/// * `secret` - The raw secret bytes
/// * `expected_hash` - The expected SHA256 hash (32 bytes)
///
/// # Returns
/// `true` if `SHA256(secret) == expected_hash`, `false` otherwise.
pub fn validate_secret(secret: &[u8], expected_hash: &[u8; 32]) -> bool {
    let hash = Sha256::digest(secret);
    hash.as_slice() == expected_hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn valid_secret_returns_true() {
        let secret = b"my_super_secret_preimage_value!!"; // 32 bytes
        let hash: [u8; 32] = Sha256::digest(secret).into();
        assert!(
            validate_secret(secret, &hash),
            "Valid secret must pass validation"
        );
    }

    #[test]
    fn invalid_secret_returns_false() {
        let secret = b"my_super_secret_preimage_value!!";
        let hash: [u8; 32] = Sha256::digest(secret).into();

        let wrong_secret = b"wrong_secret_________________!!";
        assert!(
            !validate_secret(wrong_secret, &hash),
            "Wrong secret must fail validation"
        );
    }

    #[test]
    fn empty_secret_works() {
        let secret = b"";
        let hash: [u8; 32] = Sha256::digest(secret).into();
        assert!(
            validate_secret(secret, &hash),
            "Empty secret should still validate against its hash"
        );
    }

    #[test]
    fn known_preimage_validates() {
        // Known test vector: SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let secret = b"hello";
        let expected_hash_hex = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let expected_hash: [u8; 32] = hex::decode(expected_hash_hex).unwrap().try_into().unwrap();
        assert!(validate_secret(secret, &expected_hash));
    }
}
