use crate::{
	error::Error,
	transaction::{ResubmitHandler, Transaction, TransactionStatus, TransactionsSink},
};
use async_trait::async_trait;
use futures::StreamExt;
use std::{any::Any, marker::PhantomData, pin::Pin};
use subxt::{tx::SubmittableExtrinsic, OnlineClient, PolkadotConfig};
use subxt_signer::eth::{AccountId20, Signature};

pub enum EthRuntimeConfig {}
impl subxt::Config for EthRuntimeConfig {
	type Hash = subxt::utils::H256;
	type AccountId = AccountId20;
	type Address = AccountId20;
	type Signature = Signature;
	type Hasher = subxt::config::substrate::BlakeTwo256;
	type Header =
		subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;
	type ExtrinsicParams = subxt::config::SubstrateExtrinsicParams<Self>;
	type AssetId = u32;
}

pub type TransactionSubxt<C> = SubmittableExtrinsic<C, OnlineClient<C>>;

pub type TransactionSubstrate = TransactionSubxt<PolkadotConfig>;
pub type TransactionEth = TransactionSubxt<EthRuntimeConfig>;

// todo: shall  be part of TransactionSubxt - to update mortality.
// type TransactionSubxt2 = subxt::tx::DynamicPayload;

impl<C: subxt::Config> Transaction for TransactionSubxt<C> {
	type HashType = <C as subxt::Config>::Hash;
	fn hash(&self) -> Self::HashType {
		self.hash()
	}
	fn as_any(&self) -> &dyn Any {
		self
	}
}

impl<C: subxt::Config> ResubmitHandler for TransactionSubxt<C> {
	fn handle_resubmit_request(self) -> Option<Self> {
		//mortality check and re-signing
		Some(self)
	}
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

pub struct TransactionsSinkSubxt<C: subxt::Config> {
	_p: PhantomData<C>,
}

impl<C: subxt::Config> TransactionsSinkSubxt<C> {
	pub fn new() -> Self {
		Self { _p: Default::default() }
	}
}

#[async_trait]
impl<C: subxt::Config> TransactionsSink<<C as subxt::Config>::Hash> for TransactionsSinkSubxt<C> {
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
	) -> Result<StreamOf<TransactionStatus<<C as subxt::Config>::Hash>>, Error> {
		let tx = tx.as_any().downcast_ref::<TransactionSubxt<C>>().unwrap();
		let result = tx.submit_and_watch().await;

		match result {
			Ok(stream) => Ok(stream
				.map(|e| {
					// info!(evnt=?e, "TransactionsSinkSubxt::map");
					e.unwrap().into()
				})
				.boxed()),
			Err(e) => Err(e.into()),
		}
	}

	async fn submit(
		&self,
		tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
	) -> Result<<C as subxt::Config>::Hash, Error> {
		Ok(tx.hash())
	}

	///Current count of transactions being processed by sink
	fn count(&self) -> usize {
		todo!()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::init_logger;
	use futures::StreamExt;
	use subxt::{
		config::substrate::SubstrateExtrinsicParamsBuilder as Params, dynamic::Value, OnlineClient,
	};
	use subxt_signer::eth::dev;
	use tracing::info;

	#[tokio::test]
	async fn test_subxt_send() -> Result<(), Box<dyn std::error::Error>> {
		init_logger();

		let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
			.await
			.unwrap();

		let alith = dev::alith();
		let baltathar = dev::baltathar();

		let nonce = 0;
		let tx_params = Params::new().nonce(nonce).build();

		// let tx_call = subxt::dynamic::tx("System", "remark",
		// vec![Value::from_bytes("heeelooo")]);
		let tx_call = subxt::dynamic::tx(
			"Balances",
			"transfer_keep_alive",
			vec![
				// // Substrate:
				// Value::unnamed_variant("Id", [Value::from_bytes(receiver.public())]),
				// Eth:
				Value::unnamed_composite(vec![Value::from_bytes(alith.account_id())]),
				Value::u128(1u32.into()),
			],
		);

		let tx: TransactionSubxt<EthRuntimeConfig> =
			api.tx().create_signed_offline(&tx_call, &baltathar, tx_params).unwrap();

		info!("tx hash: {:?}", tx.hash());

		let sink = TransactionsSinkSubxt::<EthRuntimeConfig>::new();

		let tx: Box<dyn Transaction<HashType = <EthRuntimeConfig as subxt::Config>::Hash>> =
			Box::from(tx);
		let mut s = sink.submit_and_watch(&*tx).await.unwrap().map(|e| info!("event: {:?}", e));
		while let Some(_) = s.next().await {}
		Ok(())
	}
}
