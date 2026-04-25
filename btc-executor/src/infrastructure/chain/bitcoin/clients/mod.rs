//! Bitcoin clients -- Electrs REST + bitcoind JSON-RPC.

pub mod bitcoind;
pub mod electrs;

pub use bitcoind::{BitcoindRpcClient, MempoolEntry, RBFTxFeeInfo};
pub use electrs::{BitcoinClientError, ElectrsClient, EsploraTx, TxStatus, TxVin, TxVout, Utxo};
