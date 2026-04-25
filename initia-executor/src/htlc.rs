use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::signers::k256::sha2::{Digest, Sha256};
use alloy::sol_types::SolValue;
use bigdecimal::{BigDecimal, ToPrimitive};

alloy::sol! {
    #[sol(rpc)]
    contract NativeHTLC {
        function initiateOnBehalf(
            address payable initiator,
            address payable redeemer,
            uint256 timelock,
            uint256 amount,
            bytes32 secretHash
        ) external payable;

        function redeem(bytes32 orderID, bytes calldata secret) external;

        function refund(bytes32 orderID) external;
    }

    #[sol(rpc)]
    contract ERC20HTLC {
        function initiateOnBehalf(
            address initiator,
            address redeemer,
            uint256 timelock,
            uint256 amount,
            bytes32 secretHash
        ) external;

        function redeem(bytes32 orderID, bytes calldata secret) external;

        function refund(bytes32 orderID) external;
    }

    #[sol(rpc)]
    contract ERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
    }
}

/// Computes the on-chain orderID matching the contract's sha256(abi.encode(...)) logic.
pub fn compute_order_id(
    chain_id: u64,
    secret_hash: FixedBytes<32>,
    initiator: Address,
    redeemer: Address,
    timelock: U256,
    amount: U256,
    htlc_address: Address,
) -> FixedBytes<32> {
    let encoded = (
        U256::from(chain_id),
        secret_hash,
        initiator,
        redeemer,
        timelock,
        amount,
        htlc_address,
    )
        .abi_encode();
    let hash = Sha256::digest(&encoded);
    FixedBytes::from_slice(&hash)
}

/// Parses a 0x-prefixed or bare hex string into a 32-byte fixed array.
pub fn parse_secret_hash(hex: &str) -> eyre::Result<FixedBytes<32>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes = alloy::hex::decode(hex)
        .map_err(|e| eyre::eyre!("Invalid hex in secret_hash: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| eyre::eyre!("secret_hash must be exactly 32 bytes"))?;
    Ok(FixedBytes::from(arr))
}

/// Parses a 0x-prefixed or bare hex string into raw `Bytes`.
pub fn parse_secret_bytes(hex: &str) -> eyre::Result<Bytes> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let bytes = alloy::hex::decode(hex)
        .map_err(|e| eyre::eyre!("Invalid hex in secret: {e}"))?;
    Ok(Bytes::from(bytes))
}

/// Parses a hex address string (with or without 0x) into an alloy `Address`.
pub fn parse_address(s: &str) -> eyre::Result<Address> {
    s.parse::<Address>()
        .map_err(|e| eyre::eyre!("Invalid address '{s}': {e}"))
}

/// Converts a `BigDecimal` representing a whole token amount in smallest units to `U256`.
pub fn bigdecimal_to_u256(val: &BigDecimal) -> eyre::Result<U256> {
    let as_u128 = val
        .to_u128()
        .ok_or_else(|| eyre::eyre!("Amount out of u128 range: {val}"))?;
    Ok(U256::from(as_u128))
}
