use crate::{
    errors::{EvmError, HTLCError},
    htlc::{common::get_instant_refund_signature, traits::HTLCInterface, NATIVE_TOKEN_ADDRESS},
    primitives::{CallParams, InitiateTypedData, SwapInfo},
    NativeHTLCv2Contract,
};
use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Address, Bytes, FixedBytes},
    providers::Provider,
    signers::{
        k256::{
            ecdsa::SigningKey,
            sha2::{self, Digest},
        },
        local::LocalSigner,
    },
    sol_types::{eip712_domain, SolValue},
};
use alloy_rpc_types_eth::AccessList;
use async_trait::async_trait;
use orderbook::primitives::EVMSwap;
use std::str::FromStr;

#[async_trait]
impl HTLCInterface for NativeHTLCv2Contract {
    async fn initiate_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .initiate(swap.redeemer, swap.timelock, swap.amount, swap.secret_hash)
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data).value(swap.amount)])
    }

    async fn initiate_on_behalf_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .initiateOnBehalf(
                swap.initiator,
                swap.redeemer,
                swap.timelock,
                swap.amount,
                swap.secret_hash,
            )
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data).value(swap.amount)])
    }

    /// Native HTLC contracts do not support the standard "initiate with signature" function.
    ///
    /// # Limitation
    /// Native HTLCs cannot be initiated via a multicall contract because the `initiate` function
    /// expects the transaction sender (`msg.sender`) to be the actual initiator. When using multicall,
    /// the sender becomes the multicall contract, not the user, which breaks the intended logic.
    ///
    /// # Why use `initiateOnBehalf`
    /// To work around this, we use the `initiateOnBehalf` function, which allows a third party
    /// (such as an executor or relayer) to initiate the swap on behalf of the original user.
    /// This function explicitly takes the initiator's address as an argument, so the correct
    /// initiator is recorded even though the transaction is sent by a different account.
    ///
    /// This is necessary because we cannot use multicall to call `initiate` directly for native HTLCs.
    async fn initiate_with_signature_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
        _domain: &Eip712Domain,
        _signer: &LocalSigner<SigningKey>,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .initiateOnBehalf(
                swap.initiator,
                swap.redeemer,
                swap.timelock,
                swap.amount,
                swap.secret_hash,
            )
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data).value(swap.amount)])
    }

    async fn initiate_with_user_signature_calldata(
        &self,
        _signature: &Bytes,
        _swap: &EVMSwap,
        _asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        Err(HTLCError::UnsupportedAction {
            action: "initiate with user signature for native htlc v2".to_string(),
        })
    }

    async fn redeem_calldata(
        &self,
        secret: &Bytes,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let transaction_request = self
            .redeem(swap.order_id, secret.clone())
            .into_transaction_request();

        let access_list = match self
            .provider()
            .create_access_list(&transaction_request)
            .await
        {
            Ok(result) => result.access_list,
            Err(e) => {
                tracing::error!("Failed to create access list: {:?}", e);
                AccessList::default()
            }
        };

        let data = transaction_request
            .input
            .with_both()
            .input
            .unwrap_or(Bytes::default());

        Ok(vec![
            CallParams::new(asset.clone(), data).access_list(access_list)
        ])
    }

    async fn refund_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self.refund(swap.order_id).calldata().clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
    }

    async fn instant_refund_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
        domain: &Eip712Domain,
        signer: &LocalSigner<SigningKey>,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let signature = get_instant_refund_signature(domain, signer, swap.order_id).await?;
        let data = self
            .instantRefund(swap.order_id, signature.clone())
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
    }

    async fn domain(&self) -> Result<Eip712Domain, HTLCError> {
        let d = self
            .eip712Domain()
            .call()
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?;
        Ok(eip712_domain! {
            name: d.name,
            version: d.version,
            chain_id: d.chainId.to(),
            verifying_contract: d.verifyingContract,
        })
    }

    async fn get_order(&self, swap: &EVMSwap) -> Result<SwapInfo, HTLCError> {
        let order = self
            .orders(swap.order_id)
            .call()
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?;

        Ok(order.into())
    }

    fn get_order_id(
        &self,
        chain_id: u64,
        swap: &EVMSwap,
        _asset: &Address,
    ) -> Result<FixedBytes<32>, HTLCError> {
        let components = (
            chain_id,
            swap.secret_hash,
            swap.initiator,
            swap.redeemer,
            swap.timelock,
            swap.amount,
        );
        let hash = sha2::Sha256::digest(components.abi_encode());
        Ok(FixedBytes::new(hash.into()))
    }

    async fn token(&self) -> Result<Address, HTLCError> {
        Ok(Address::from_str(NATIVE_TOKEN_ADDRESS)
            .map_err(|e| HTLCError::EvmError(EvmError::DecodeAddressError(e)))?)
    }

    async fn get_typed_data(
        &self,
        _swap: &EVMSwap,
        _domain: Eip712Domain,
    ) -> Result<InitiateTypedData, HTLCError> {
        Err(HTLCError::UnsupportedAction {
            action: "get_typed_data not supported for native htlc v2".to_string(),
        })
    }

    fn clone_box(&self) -> Box<dyn HTLCInterface + Send + Sync> {
        Box::new(self.clone())
    }
}
