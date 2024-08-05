use crate::{
	cli::AccountsDescription,
	error::Error,
	transaction::{
		AccountMetadata, ResubmitHandler, Transaction, TransactionStatus, TransactionsSink,
	},
};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use std::{any::Any, collections::HashMap, hash::Hash, marker::PhantomData, pin::Pin, sync::Arc};
use subxt::{
	config::signed_extensions::{
		ChargeAssetTxPaymentParams, ChargeTransactionPaymentParams, CheckMortalityParams,
		CheckNonceParams,
	},
	dynamic::{At, Value},
	tx::{Signer, SubmittableExtrinsic},
	OnlineClient, PolkadotConfig,
};
use subxt_core::config::SubstrateExtrinsicParamsBuilder;
use subxt_signer::eth::{dev, AccountId20, Keypair as EthKeypair, Signature};
use tracing::{debug, info, trace};

const LOG_TARGET: &str = "subxt_tx";

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

pub type HashOf<C> = <C as subxt::Config>::Hash;
pub type AccountIdOf<C> = <C as subxt::Config>::AccountId;

pub struct TransactionSubxt<C: subxt::Config> {
	extrinsic: SubmittableExtrinsic<C, OnlineClient<C>>,
	nonce: u128,
	account_metadata: AccountMetadata,
}

pub type EthTransaction = TransactionSubxt<EthRuntimeConfig>;
pub type EthTransactionsSink = SubxtTransactionsSink<EthRuntimeConfig, EthKeypair>;

impl<C: subxt::Config> TransactionSubxt<C> {
	pub fn new(
		extrinsic: SubmittableExtrinsic<C, OnlineClient<C>>,
		nonce: u128,
		account_metadata: AccountMetadata,
	) -> Self {
		Self { extrinsic, nonce, account_metadata }
	}
}

pub type TransactionSubstrate = TransactionSubxt<PolkadotConfig>;
pub type TransactionEth = TransactionSubxt<EthRuntimeConfig>;

// todo: shall  be part of TransactionSubxt - to update mortality.
// type TransactionSubxt2 = subxt::tx::DynamicPayload;

impl<C: subxt::Config> Transaction for TransactionSubxt<C> {
	type HashType = <C as subxt::Config>::Hash;
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

impl<C: subxt::Config> ResubmitHandler for TransactionSubxt<C> {
	fn handle_resubmit_request(self) -> Option<Self> {
		//mortality check and re-signing
		Some(self)
	}
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

pub struct SubxtTransactionsSink<C: subxt::Config, KP: Signer<C>> {
	api: OnlineClient<C>,
	from_accounts: Arc<RwLock<HashMap<String, KP>>>,
	to_accounts: Arc<RwLock<HashMap<String, KP>>>,
	nonces: Arc<RwLock<HashMap<String, u128>>>,
}

impl<C, KP> SubxtTransactionsSink<C, KP>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
	KP: Signer<C> + Clone + Send + Sync + 'static,
	C: subxt::Config,
{
	pub async fn new() -> Self {
		Self {
			api: OnlineClient::<C>::from_insecure_url("ws://127.0.0.1:9933").await.unwrap(),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
		}
	}

	pub async fn new_with_uri(uri: &String) -> Self {
		Self {
			api: OnlineClient::<C>::from_insecure_url(uri).await.unwrap(),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
		}
	}

	pub async fn new_with_uri_with_accounts_description<G>(
		uri: &String,
		accounts_description: AccountsDescription,
		generate_pair: G,
	) -> Self
	where
		G: Fn(&String) -> KP + Copy + Send + 'static,
	{
		let from_accounts =
			derive_accounts(accounts_description.clone(), &SENDER_SEED, generate_pair);
		let to_accounts = derive_accounts(accounts_description, &RECEIVER_SEED, generate_pair);
		Self {
			// api: OnlineClient::<C>::from_insecure_url(uri).await.unwrap(),
			api: crate::subxt_api_connector::connect(uri)
				.await
				.expect("connecting to node should not fail"),
			from_accounts: Arc::from(RwLock::from(from_accounts)),
			to_accounts: Arc::from(RwLock::from(to_accounts)),
			nonces: Default::default(),
		}
	}

	fn api(&self) -> OnlineClient<C> {
		self.api.clone()
	}

	fn get_from_account_id(&self, account: &String) -> Option<AccountIdOf<C>> {
		self.from_accounts.read().get(account).map(|a| a.account_id())
	}

	fn get_to_account_id(&self, account: &String) -> Option<AccountIdOf<C>> {
		self.to_accounts.read().get(account).map(|a| a.account_id())
	}

	fn get_from_key_pair(&self, account: &String) -> Option<KP> {
		self.from_accounts.read().get(account).cloned()
	}

	async fn check_account_nonce(
		&self,
		account: AccountIdOf<C>,
	) -> Result<u128, Box<dyn std::error::Error>> {
		if let Some(nonce) = self.nonces.write().get_mut(&hex::encode(account.clone())) {
			*nonce = *nonce + 1;
			return Ok(*nonce)
		}

		{
			let storage_query = subxt::dynamic::storage(
				"System",
				"Account",
				vec![Value::from_bytes(account.clone())],
			);
			let result = self.api.storage().at_latest().await?.fetch(&storage_query).await?;
			let value = result
				.ok_or(format!("Sender account {:?} shall exists", hex::encode(account.clone())))?
				.to_value()?;

			debug!(target:LOG_TARGET,"account has free balance: {:?}", value.at("data").at("free"));
			debug!(target:LOG_TARGET,"account has nonce: {:?}", value.at("nonce"));
			// info!("account has nonce: {:#?}", value);
			let nonce = value
				.at("nonce")
				.expect("nonce shall be there")
				.as_u128()
				.expect("shall be u128");

			self.nonces.write().insert(hex::encode(account), nonce);
			Ok(nonce)
		}
	}
}

#[async_trait]
impl<C, KP> TransactionsSink<<C as subxt::Config>::Hash> for SubxtTransactionsSink<C, KP>
where
	AccountIdOf<C>: Send + Sync,
	C: subxt::Config,
	KP: Signer<C> + Send + Sync + 'static,
{
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
	) -> Result<StreamOf<TransactionStatus<<C as subxt::Config>::Hash>>, Error> {
		let tx = tx.as_any().downcast_ref::<TransactionSubxt<C>>().unwrap();
		let result = tx.extrinsic.submit_and_watch().await;

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

const SENDER_SEED: &str = "//Sender";
const RECEIVER_SEED: &str = "//Receiver";
const SEED: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

pub fn generate_ecdsa_keypair(derivation: &String) -> EthKeypair {
	match derivation.as_str() {
		"alice" | "alith" => dev::alith(),
		"bob" | "baltathar" => dev::baltathar(),
		"charlie" | "charleth" => dev::charleth(),
		"dave" | "dorothy" => dev::dorothy(),
		"eve" | "ethan" => dev::ethan(),
		"ferdie" | "faith" => dev::faith(),
		_ => {
			use std::str::FromStr;
			let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
			<subxt_signer::ecdsa::Keypair>::from_uri(&u).unwrap().into()
		},
	}
}

pub fn derive_accounts<C, KP, G>(
	accounts_description: AccountsDescription,
	seed: &str,
	generate: G,
) -> HashMap<String, KP>
where
	C: subxt::Config,
	KP: Signer<C> + Send + Sync + 'static,
	G: Fn(&String) -> KP + Copy + Send + 'static,
{
	match accounts_description {
		AccountsDescription::Derived(range) => {
			let from_id = range.start as usize;
			let to_id = range.end as usize;
			let n = to_id - from_id;
			let t = std::cmp::min(
				n,
				std::thread::available_parallelism().unwrap_or(1usize.try_into().unwrap()).get(),
			);
			let mut threads = Vec::new();

			(0..t).into_iter().for_each(|thread_idx| {
				// let chunk = (thread_idx * (n / t))..((thread_idx + 1) * (n / t));
				let chunk =
					(from_id + (thread_idx * n) / t)..(from_id + ((thread_idx + 1) * n) / t);
				let seed = seed.to_string().clone();
				threads.push(std::thread::spawn(move || {
					chunk
						.into_iter()
						.map(move |i| {
							let derivation = format!("{SEED}{seed}//{i}");
							// info!("derivation: {thread_idx:} {derivation:?}");
							(i.to_string(), generate(&derivation))
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
		},
		AccountsDescription::Keyring(account) =>
			HashMap::from([(account.clone(), generate(&account))]),
	}
}

pub async fn build_subxt_tx<C, KP>(
	account: &String,
	nonce: &Option<u128>,
	sink: &SubxtTransactionsSink<C, KP>,
) -> TransactionSubxt<C>
where
	AccountIdOf<C>: Send + Sync,
	AccountIdOf<C>: AsRef<[u8]>,
	C: subxt::Config,
	KP: Signer<C> + Clone + Send + Sync + 'static,
	<<C as subxt::Config>::ExtrinsicParams as subxt::config::ExtrinsicParams<C>>::Params: From<(
		(),
		(),
		CheckNonceParams,
		(),
		CheckMortalityParams<C>,
		ChargeAssetTxPaymentParams<C>,
		ChargeTransactionPaymentParams,
		(),
	)>,
{
	//todo:
	let to_account_id = sink.get_to_account_id(account).expect("to account exists");
	let from_account_id = sink.get_from_account_id(account).expect("from account exists");
	let from_keypair = sink.get_from_key_pair(account).expect("from account exists");

	let nonce = if let Some(nonce) = nonce {
		debug!("nonce for {:?} -> {:?}", account, nonce);
		*nonce
	} else {
		let nonce = sink
			.check_account_nonce(from_account_id)
			.await
			.expect("account nonce shall exists");
		debug!("checked nonce for {:?} -> {:?}", account, nonce);
		nonce
	};
	let tx_params = <SubstrateExtrinsicParamsBuilder<C>>::new().nonce(nonce as u64).build().into();

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

	let tx = TransactionSubxt::<C>::new(
		sink.api()
			.tx()
			.create_signed_offline(&tx_call, &from_keypair, tx_params)
			.unwrap(),
		nonce as u128,
		AccountMetadata::KeyRing("baltathar".to_string()),
	);

	debug!(target:LOG_TARGET,"tx hash: {:?}", tx.hash());

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
