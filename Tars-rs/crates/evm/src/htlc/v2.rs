use crate::{
    errors::{EvmError, HTLCError},
    htlc::{common::get_instant_refund_signature, traits::HTLCInterface, v1::Initiate},
    primitives::{CallParams, InitiateTypedData, SwapInfo},
    UnipayHTLCv2Contract,
};
use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Address, Bytes, FixedBytes},
    signers::{
        k256::{
            ecdsa::SigningKey,
            sha2::{self, Digest},
        },
        local::LocalSigner,
        Signer,
    },
    sol_types::{eip712_domain, SolValue},
};
use async_trait::async_trait;
use orderbook::primitives::EVMSwap;
use serde_json::json;

impl UnipayHTLCv2Contract {
    async fn get_initiate_signature(
        &self,
        swap: &EVMSwap,
        domain: &Eip712Domain,
        signer: &LocalSigner<SigningKey>,
    ) -> Result<Bytes, HTLCError> {
        let initiate: Initiate = swap.into();
        let signature = signer
            .sign_typed_data(&initiate, &domain)
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::SignatureError(e)))?;
        Ok(Bytes::from(signature.as_bytes()))
    }
}

#[async_trait]
impl HTLCInterface for UnipayHTLCv2Contract {
    async fn initiate_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .initiate(swap.redeemer, swap.timelock, swap.amount, swap.secret_hash)
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
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

        Ok(vec![CallParams::new(asset.clone(), data)])
    }

    async fn initiate_with_signature_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
        domain: &Eip712Domain,
        signer: &LocalSigner<SigningKey>,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let signature = self.get_initiate_signature(swap, domain, signer).await?;
        let data = self
            .initiateWithSignature(
                swap.initiator,
                swap.redeemer,
                swap.timelock,
                swap.amount,
                swap.secret_hash,
                signature.clone(),
            )
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
    }

    async fn initiate_with_user_signature_calldata(
        &self,
        signature: &Bytes,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .initiateWithSignature(
                swap.initiator,
                swap.redeemer,
                swap.timelock,
                swap.amount,
                swap.secret_hash,
                signature.clone(),
            )
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
    }

    async fn redeem_calldata(
        &self,
        secret: &Bytes,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let data = self
            .redeem(swap.order_id, secret.clone())
            .calldata()
            .clone();

        Ok(vec![CallParams::new(asset.clone(), data)])
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
        self.token()
            .call()
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))
    }

    async fn get_typed_data(
        &self,
        swap: &EVMSwap,
        domain: Eip712Domain,
    ) -> Result<InitiateTypedData, HTLCError> {
        let initiate: Initiate = swap.into();

        // Hardcoded EIP712 types structure
        let types = json!({
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "version", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ],
            "Initiate": [
                {"name": "redeemer", "type": "address"},
                {"name": "timelock", "type": "uint256"},
                {"name": "amount", "type": "uint256"},
                {"name": "secretHash", "type": "bytes32"}
            ]
        });

        Ok(InitiateTypedData {
            domain,
            primary_type: "Initiate".to_string(),
            types,
            message: json!(initiate),
        })
    }

    fn clone_box(&self) -> Box<dyn HTLCInterface + Send + Sync> {
        Box::new(self.clone())
    }
}
