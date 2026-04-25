use std::str::FromStr;
use std::sync::Arc;

use bitcoin::Address;
use bitcoin::address::NetworkChecked;
use tars::bitcoin::htlc::validate::validate_instant_refund_sacp_tx;
use tars::bitcoin::{HTLCParams as TarsHtlcParams, Utxo as TarsUtxo, UtxoStatus as TarsUtxoStatus};
use tars::orderbook::primitives::{MatchedOrderVerbose, SingleSwap};
use tars::primitives::HTLCAction;

use crate::errors::{ChainError, ExecutorError};
use crate::infrastructure::chain::bitcoin::clients::ElectrsClient;
use crate::infrastructure::chain::bitcoin::primitives::HTLCParams;
use crate::infrastructure::chain::bitcoin::wallet::{
    BitcoinHtlcWalletAdapter, HtlcAction, WalletRequestSubmitter,
};
use crate::infrastructure::keys::BitcoinWallet;

pub struct BitcoinActionExecutor {
    wallet: Arc<BitcoinWallet>,
    submitter: Arc<dyn WalletRequestSubmitter>,
    electrs: Arc<ElectrsClient>,
    adapter: BitcoinHtlcWalletAdapter,
    network: bitcoin::Network,
}

impl BitcoinActionExecutor {
    pub fn new(
        wallet: Arc<BitcoinWallet>,
        submitter: Arc<dyn WalletRequestSubmitter>,
        electrs: Arc<ElectrsClient>,
        network: bitcoin::Network,
    ) -> Self {
        let adapter =
            BitcoinHtlcWalletAdapter::new(*wallet.x_only_pubkey(), Arc::clone(&electrs), network);
        Self {
            wallet,
            submitter,
            electrs,
            adapter,
            network,
        }
    }

    pub fn solver_id(&self) -> String {
        hex::encode(self.wallet.x_only_pubkey().serialize())
    }

    pub fn solver_address(&self) -> String {
        self.wallet.address().to_string()
    }

    pub async fn current_block_height(&self) -> Result<u64, ExecutorError> {
        self.electrs
            .get_block_height()
            .await
            .map_err(|err| ExecutorError::Chain(ChainError::Rpc(err.to_string())))
    }

    pub async fn execute_action(
        &self,
        order: &MatchedOrderVerbose,
        action: &HTLCAction,
        swap: &SingleSwap,
    ) -> Result<usize, ExecutorError> {
        let requests = self.prepare_requests(order, action, swap).await?;
        let request_count = requests.len();

        for request in requests {
            self.submitter.submit(request).await?;
        }

        Ok(request_count)
    }

    async fn prepare_requests(
        &self,
        order: &MatchedOrderVerbose,
        action: &HTLCAction,
        swap: &SingleSwap,
    ) -> Result<Vec<crate::infrastructure::chain::bitcoin::wallet::WalletRequest>, ExecutorError>
    {
        let params = swap_to_htlc_params(swap)?;
        let htlc_address = self.resolve_htlc_address(swap, &params)?;

        match action {
            HTLCAction::Initiate => self
                .adapter
                .prepare(HtlcAction::Initiate {
                    dedupe_key: format!("initiate:{}", swap.swap_id),
                    htlc_address,
                    amount: params.amount,
                })
                .await
                .map_err(adapter_error),
            HTLCAction::Redeem { secret } => self
                .adapter
                .prepare(HtlcAction::Redeem {
                    dedupe_key: format!("redeem:{}", swap.swap_id),
                    htlc_address,
                    params,
                    secret: secret.to_vec(),
                })
                .await
                .map_err(adapter_error),
            HTLCAction::Refund => self
                .adapter
                .prepare(HtlcAction::Refund {
                    dedupe_key: format!("refund:{}", swap.swap_id),
                    htlc_address,
                    params,
                })
                .await
                .map_err(adapter_error),
            HTLCAction::InstantRefund => {
                let instant_refund_tx_hex = order
                    .create_order
                    .additional_data
                    .instant_refund_tx_bytes
                    .clone()
                    .ok_or_else(|| {
                        ExecutorError::Chain(ChainError::ValidationFailed(format!(
                            "order {} missing instant_refund_tx_bytes",
                            order.create_order.create_id
                        )))
                    })?;
                let recipient =
                    self.parse_address(&order.get_bitcoin_recipient_address().map_err(|err| {
                        ExecutorError::Chain(ChainError::ValidationFailed(format!(
                            "order {} missing bitcoin recipient: {err}",
                            order.create_order.create_id
                        )))
                    })?)?;
                self.validate_instant_refund_tx(&instant_refund_tx_hex, swap, &params, &recipient)
                    .await?;
                self.adapter
                    .prepare(HtlcAction::InstantRefund {
                        dedupe_key_prefix: format!("instant_refund:{}", swap.swap_id),
                        htlc_address,
                        params,
                        recipient,
                        instant_refund_tx_hex,
                    })
                    .await
                    .map_err(adapter_error)
            }
            other => Err(ExecutorError::Chain(ChainError::Unsupported(format!(
                "bitcoin executor does not support action {other}"
            )))),
        }
    }

    async fn validate_instant_refund_tx(
        &self,
        instant_refund_tx_hex: &str,
        swap: &SingleSwap,
        params: &HTLCParams,
        recipient: &Address<NetworkChecked>,
    ) -> Result<(), ExecutorError> {
        let utxos = self
            .electrs
            .get_address_utxos(&self.resolve_htlc_address(swap, params)?.to_string())
            .await
            .map_err(|err| ExecutorError::Chain(ChainError::Rpc(err.to_string())))?
            .into_iter()
            .map(|utxo| {
                let txid = bitcoin::Txid::from_str(&utxo.txid).map_err(|err| {
                    ExecutorError::Chain(ChainError::DecodeFailed(format!(
                        "invalid electrs txid for instant refund: {err}"
                    )))
                })?;
                Ok(TarsUtxo {
                    txid,
                    vout: utxo.vout,
                    value: utxo.value,
                    status: TarsUtxoStatus {
                        confirmed: utxo.status.confirmed,
                        block_height: utxo.status.block_height,
                    },
                })
            })
            .collect::<Result<Vec<_>, ExecutorError>>()?;

        validate_instant_refund_sacp_tx(
            instant_refund_tx_hex,
            &swap.get_init_tx_hash().map_err(|err| {
                ExecutorError::Chain(ChainError::ValidationFailed(format!(
                    "invalid initiate tx hash on swap {}: {err}",
                    swap.swap_id
                )))
            })?,
            &TarsHtlcParams {
                initiator_pubkey: params.initiator_pubkey,
                redeemer_pubkey: params.redeemer_pubkey,
                amount: params.amount,
                secret_hash: params.secret_hash,
                timelock: params.timelock,
            },
            &utxos,
            recipient,
            self.network,
        )
        .map_err(|err| {
            ExecutorError::Chain(ChainError::ValidationFailed(format!(
                "invalid instant refund tx for order {}: {err}",
                swap.swap_id
            )))
        })
    }

    fn resolve_htlc_address(
        &self,
        swap: &SingleSwap,
        _params: &HTLCParams,
    ) -> Result<Address<NetworkChecked>, ExecutorError> {
        self.parse_address(&swap.swap_id)
    }

    fn parse_address(&self, address: &str) -> Result<Address<NetworkChecked>, ExecutorError> {
        let parsed = Address::from_str(address).map_err(|err| {
            ExecutorError::Chain(ChainError::ValidationFailed(format!(
                "invalid bitcoin address {address}: {err}"
            )))
        })?;

        parsed.require_network(self.network).map_err(|err| {
            ExecutorError::Chain(ChainError::ValidationFailed(format!(
                "bitcoin address {address} does not match network {:?}: {err}",
                self.network
            )))
        })
    }
}

fn swap_to_htlc_params(swap: &SingleSwap) -> Result<HTLCParams, ExecutorError> {
    let initiator_pubkey = bitcoin::XOnlyPublicKey::from_str(&swap.initiator).map_err(|err| {
        ExecutorError::Chain(ChainError::ValidationFailed(format!(
            "invalid initiator pubkey for swap {}: {err}",
            swap.swap_id
        )))
    })?;
    let redeemer_pubkey = bitcoin::XOnlyPublicKey::from_str(&swap.redeemer).map_err(|err| {
        ExecutorError::Chain(ChainError::ValidationFailed(format!(
            "invalid redeemer pubkey for swap {}: {err}",
            swap.swap_id
        )))
    })?;
    let secret_hash = hex::decode(&swap.secret_hash)
        .map_err(|err| {
            ExecutorError::Chain(ChainError::ValidationFailed(format!(
                "invalid secret hash for swap {}: {err}",
                swap.swap_id
            )))
        })?
        .try_into()
        .map_err(|_| {
            ExecutorError::Chain(ChainError::ValidationFailed(format!(
                "secret hash has invalid length for swap {}",
                swap.swap_id
            )))
        })?;
    let amount = swap.amount.to_string().parse::<u64>().map_err(|err| {
        ExecutorError::Chain(ChainError::ValidationFailed(format!(
            "invalid bitcoin amount for swap {}: {err}",
            swap.swap_id
        )))
    })?;

    Ok(HTLCParams {
        initiator_pubkey,
        redeemer_pubkey,
        amount,
        secret_hash,
        timelock: swap.timelock as u64,
    })
}

fn adapter_error(
    err: crate::infrastructure::chain::bitcoin::wallet::HtlcAdapterError,
) -> ExecutorError {
    ExecutorError::Chain(ChainError::Other(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    use crate::infrastructure::chain::bitcoin::wallet::WalletRequest;
    use crate::infrastructure::chain::bitcoin::wallet::runner::ResolvePendingRequestResult;

    const TEST_BTC_PRIVKEY_HEX: &str =
        "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35";

    struct NoopSubmitter;

    #[async_trait]
    impl WalletRequestSubmitter for NoopSubmitter {
        async fn submit(&self, _request: WalletRequest) -> Result<(), ExecutorError> {
            unimplemented!("submit is not used in this test")
        }

        async fn submit_and_wait(
            &self,
            _request: WalletRequest,
        ) -> Result<bitcoin::Txid, ExecutorError> {
            unimplemented!("submit_and_wait is not used in this test")
        }

        async fn resolve_pending(
            &self,
            _dedupe_key: &str,
        ) -> Result<ResolvePendingRequestResult, ExecutorError> {
            unimplemented!("resolve_pending is not used in this test")
        }
    }

    #[test]
    fn exposes_solver_taproot_address() {
        let wallet = Arc::new(
            BitcoinWallet::from_private_key(TEST_BTC_PRIVKEY_HEX, bitcoin::Network::Testnet)
                .expect("wallet"),
        );
        let expected_address = wallet.address().to_string();
        let executor = BitcoinActionExecutor::new(
            wallet,
            Arc::new(NoopSubmitter),
            Arc::new(ElectrsClient::new("http://127.0.0.1:30000".to_string())),
            bitcoin::Network::Testnet,
        );

        assert_eq!(executor.solver_address(), expected_address);
    }
}
