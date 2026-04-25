//! Bitcoin HTLC primitives -- Taproot scripts, witnesses, sighash, validation.
//!
//! Vendored from garden-rs and adapted to remove garden-specific dependencies.
//! Only depends on: `bitcoin`, `secp256k1`, `sha2`, `hex`, `thiserror`.

pub mod error;
pub mod hash;
pub mod htlc;
pub mod htlc_id;
pub mod script;
pub mod sig;
pub mod validate;
pub mod witness;

// Re-exports for convenient access
pub use error::BitcoinPrimitivesError;
pub use hash::TapScriptSpendSigHashGenerator;
pub use htlc::{
    construct_taproot_spend_info, get_control_block, get_htlc_address, get_htlc_leaf_hash,
    get_htlc_leaf_script, HTLCLeaf, HTLCParams, GARDEN_NUMS,
};
pub use htlc_id::compute_bitcoin_htlc_id;
pub use script::{instant_refund_leaf, redeem_leaf, refund_leaf};
pub use sig::add_signature_to_witness;
pub use validate::validate_secret;
pub use witness::{get_instant_refund_witness, get_redeem_witness, get_refund_witness};
