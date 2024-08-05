use crate::{
	error::Error,
	transaction::{
		AccountMetadata, ResubmitHandler, Transaction, TransactionStatus, TransactionsSink,
	},
};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use std::{any::Any, collections::HashMap, marker::PhantomData, pin::Pin, sync::Arc};
use subxt::{
	config::BlockHash,
	dynamic::{At, Value},
	tx::{Signer, SubmittableExtrinsic},
	OnlineClient, PolkadotConfig,
};
use subxt_core::config::SubstrateExtrinsicParamsBuilder as Params;
use subxt_signer::eth::{dev, AccountId20, Keypair as EthKeypair, Signature};
use tracing::{info, trace};

const LOG_TARGET: &str = "eth_tx";

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

pub struct EthTransaction {
	extrinsic: SubmittableExtrinsic<EthRuntimeConfig, OnlineClient<EthRuntimeConfig>>,
	nonce: u128,
	account_metadata: AccountMetadata,
}

impl EthTransaction {
	pub fn new(
		extrinsic: SubmittableExtrinsic<EthRuntimeConfig, OnlineClient<EthRuntimeConfig>>,
		nonce: u128,
		account_metadata: AccountMetadata,
	) -> Self {
		Self { extrinsic, nonce, account_metadata }
	}
}

impl Transaction for EthTransaction {
	type HashType = HashOf<EthRuntimeConfig>;
	fn hash(&self) -> Self::HashType {
		self.extrinsic.hash()
	}
	fn as_any(&self) -> &dyn Any {
		self
	}
	fn nonce(&self) -> u128 {
		self.nonce
	}
	fn account_metadata(&self) -> AccountMetadata {
		self.account_metadata.clone()
	}
}

impl ResubmitHandler for EthTransaction {
	fn handle_resubmit_request(self) -> Option<Self> {
		//mortality check and re-signing
		Some(self)
	}
}

pub type HashOf<C> = <C as subxt::Config>::Hash;
type AccountIdOf<C> = <C as subxt::Config>::AccountId;

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

pub struct EthTransactionsSink {
	api: OnlineClient<EthRuntimeConfig>,
	accounts: Arc<RwLock<HashMap<String, EthKeypair>>>,
}

impl EthTransactionsSink {
	pub async fn new() -> Self {
		Self {
			api: OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
				.await
				.unwrap(),
			accounts: Default::default(),
		}
	}

	pub async fn new_with_uri(uri: &String) -> Self {
		Self {
			api: OnlineClient::<EthRuntimeConfig>::from_insecure_url(uri).await.unwrap(),
			accounts: Default::default(),
		}
	}

	fn api(&self) -> OnlineClient<EthRuntimeConfig> {
		self.api.clone()
	}

	fn get_account_id(&self, account: &String) -> Option<AccountIdOf<EthRuntimeConfig>> {
		self.accounts.read().get(account).map(|a| a.account_id())
	}

	fn get_key_pair(&self, account: &String) -> Option<EthKeypair> {
		self.accounts.read().get(account).cloned()
	}

	async fn check_account_nonce(
		&self,
		account: AccountIdOf<EthRuntimeConfig>,
	) -> Result<u128, Box<dyn std::error::Error>> {
		let storage_query =
			subxt::dynamic::storage("System", "Account", vec![Value::from_bytes(account)]);
		let result = self.api.storage().at_latest().await?.fetch(&storage_query).await?;
		let value = result.unwrap().to_value()?;

		trace!(target:LOG_TARGET,"account has free balance: {:?}", value.at("data").at("free"));
		trace!(target:LOG_TARGET,"account has nonce: {:?}", value.at("nonce"));
		// info!("account has nonce: {:#?}", value);
		Ok(value
			.at("nonce")
			.expect("nonce shall be there")
			.as_u128()
			.expect("shall be u128"))
	}
}

#[async_trait]
impl TransactionsSink<HashOf<EthRuntimeConfig>> for EthTransactionsSink {
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = HashOf<EthRuntimeConfig>>,
	) -> Result<StreamOf<TransactionStatus<HashOf<EthRuntimeConfig>>>, Error> {
		let tx = tx.as_any().downcast_ref::<EthTransaction>().unwrap();
		let result = tx.extrinsic.submit_and_watch().await;

		match result {
			Ok(stream) => Ok(stream
				.map(|e| {
					// info!(evnt=?e, "TransactionsSinkSubxt::map");<H: BlockHash>
					e.unwrap().into()
				})
				.boxed()),
			Err(e) => Err(e.into()),
		}
	}

	async fn submit(
		&self,
		tx: &dyn Transaction<HashType = HashOf<EthRuntimeConfig>>,
	) -> Result<HashOf<EthRuntimeConfig>, Error> {
		Ok(tx.hash())
	}

	///Current count of transactions being processed by sink
	fn count(&self) -> usize {
		todo!()
	}
}

const SENDER_SEED: &str = "//Sender";
const RECEIVER_SEED: &str = "//Receiver";
const SEED: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

pub fn derive_accounts(n: usize, seed: String) -> Vec<(usize, EthKeypair)> {
	let t = std::cmp::min(
		n,
		std::thread::available_parallelism().unwrap_or(1usize.try_into().unwrap()).get(),
	);
	let mut threads = Vec::new();

	(0..t).into_iter().for_each(|thread_idx| {
		let chunk = (thread_idx * (n / t))..((thread_idx + 1) * (n / t));
		let seed = seed.clone();
		threads.push(std::thread::spawn(move || {
			chunk
				.into_iter()
				.map(move |i| {
					let derivation = format!("{SEED}{seed}//{i}");
					// info!("derivation: {thread_idx:} {derivation:?}");
					use std::str::FromStr;
					let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
					(i, <subxt_signer::ecdsa::Keypair>::from_uri(&u).unwrap().into())
				})
				.collect::<Vec<_>>()
		}));
	});

	threads
		.into_iter()
		.map(|h| h.join().unwrap())
		.flatten()
		// .map(|p| (p, funds))
		.collect()
}

pub async fn build_eth_tx(
	account: &String,
	nonce: &Option<u128>,
	sink: &EthTransactionsSink,
) -> EthTransaction {
	//todo:
	let to_account_id = sink.get_account_id(&"alice".to_string()).expect("to account exists");
	let from_account_id = sink.get_account_id(account).expect("from account exists");
	let from_keypair = sink.get_key_pair(account).expect("from account exists");

	let nonce = if let Some(nonce) = nonce {
		*nonce
	} else {
		sink.check_account_nonce(from_account_id)
			.await
			.expect("account nonce shall exists")
	};

	let tx_params = Params::new().nonce(nonce as u64).build();

	// let tx_call = subxt::dynamic::tx("System", "remark",
	// vec![Value::from_bytes("heeelooo")]);

	let tx_call = subxt::dynamic::tx(
		"Balances",
		"transfer_keep_alive",
		vec![
			// // Substrate:
			// Value::unnamed_variant("Id", [Value::from_bytes(receiver.public())]),
			// Eth:
			Value::unnamed_composite(vec![Value::from_bytes(to_account_id)]),
			Value::u128(1u32.into()),
		],
	);

	let tx = EthTransaction::new(
		sink.api()
			.tx()
			.create_signed_offline(&tx_call, &from_keypair, tx_params)
			.unwrap(),
		nonce as u128,
		AccountMetadata::KeyRing("baltathar".to_string()),
	);

	trace!(target:LOG_TARGET,"tx hash: {:?}", tx.hash());

	tx
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

		let tx = TransactionSubxt::<EthRuntimeConfig>::new(
			api.tx().create_signed_offline(&tx_call, &baltathar, tx_params).unwrap(),
			nonce as u128,
			AccountMetadata::KeyRing("baltathar".to_string()),
		);

		info!("tx hash: {:?}", tx.hash());

		let sink = SubxtTransactionsSink::<EthRuntimeConfig>::new().await;

		let tx: Box<dyn Transaction<HashType = <EthRuntimeConfig as subxt::Config>::Hash>> =
			Box::from(tx);
		let mut s = sink.submit_and_watch(&*tx).await.unwrap().map(|e| info!("event: {:?}", e));
		while let Some(_) = s.next().await {}
		Ok(())
	}
}
