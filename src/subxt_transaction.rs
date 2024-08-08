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
use std::{
	any::Any,
	collections::HashMap,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};
use subxt::{
	backend::rpc::RpcClient,
	config::signed_extensions::{
		ChargeAssetTxPaymentParams, ChargeTransactionPaymentParams, CheckMortalityParams,
		CheckNonceParams,
	},
	dynamic::{At, Value},
	rpc_params,
	tx::{DynamicPayload, Signer, SubmittableExtrinsic},
	OnlineClient, PolkadotConfig,
};
use subxt_core::config::SubstrateExtrinsicParamsBuilder;
use subxt_signer::{
	eth::{dev as eth_dev, AccountId20, Keypair as EthKeypair, Signature},
	sr25519::{dev as sr25519_dev, Keypair as SrPair},
};
use tracing::{debug, trace};

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

pub struct SubxtTransaction<C: subxt::Config> {
	extrinsic: SubmittableExtrinsic<C, OnlineClient<C>>,
	nonce: u128,
	account_metadata: AccountMetadata,
}

pub type EthTransaction = SubxtTransaction<EthRuntimeConfig>;
pub type EthTransactionsSink = SubxtTransactionsSink<EthRuntimeConfig, EthKeypair>;
pub type SubstrateTransaction = SubxtTransaction<PolkadotConfig>;
pub type SubstrateTransactionsSink = SubxtTransactionsSink<PolkadotConfig, SrPair>;

impl<C: subxt::Config> SubxtTransaction<C> {
	pub fn new(
		extrinsic: SubmittableExtrinsic<C, OnlineClient<C>>,
		nonce: u128,
		account_metadata: AccountMetadata,
	) -> Self {
		Self { extrinsic, nonce, account_metadata }
	}
}

// todo: shall  be part of TransactionSubxt - to update mortality.
// type TransactionSubxt2 = subxt::tx::DynamicPayload;

impl<C: subxt::Config> Transaction for SubxtTransaction<C> {
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

impl<C: subxt::Config> ResubmitHandler for SubxtTransaction<C> {
	fn handle_resubmit_request(self) -> Option<Self> {
		//mortality check and re-signing
		Some(self)
	}
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

pub struct SubxtTransactionsSink<C: subxt::Config, KP: Signer<C>> {
	api: OnlineClient<C>,
	from_accounts: Arc<RwLock<HashMap<String, (KP, AccountMetadata)>>>,
	to_accounts: Arc<RwLock<HashMap<String, (KP, AccountMetadata)>>>,
	nonces: Arc<RwLock<HashMap<String, u128>>>,
	rpc_client: RpcClient,
	current_pending_extrinsics: RwLock<Option<(Instant, usize)>>,
}

const EXPECT_CONNECT: &str = "should connect to rpc client";

impl<C, KP> SubxtTransactionsSink<C, KP>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
	KP: Signer<C> + Clone + Send + Sync + 'static,
	C: subxt::Config,
{
	pub async fn new() -> Self {
		Self {
			api: OnlineClient::<C>::from_insecure_url("ws://127.0.0.1:9933")
				.await
				.expect(EXPECT_CONNECT),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
			rpc_client: RpcClient::from_url("ws://127.0.0.1:9933").await.expect(EXPECT_CONNECT),
			current_pending_extrinsics: None.into(),
		}
	}

	pub async fn new_with_uri(uri: &String) -> Self {
		Self {
			api: OnlineClient::<C>::from_insecure_url(uri).await.expect(EXPECT_CONNECT),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
			rpc_client: RpcClient::from_url(uri).await.expect(EXPECT_CONNECT),
			current_pending_extrinsics: None.into(),
		}
	}

	pub async fn new_with_uri_with_accounts_description<G>(
		uri: &String,
		accounts_description: AccountsDescription,
		generate_pair: G,
	) -> Self
	where
		G: GenerateKeyPairFunction<KP>,
	{
		let from_accounts =
			derive_accounts(accounts_description.clone(), &SENDER_SEED, generate_pair);
		let to_accounts = derive_accounts(accounts_description, &RECEIVER_SEED, generate_pair);
		Self {
			api: crate::subxt_api_connector::connect(uri).await.expect(EXPECT_CONNECT),
			from_accounts: Arc::from(RwLock::from(from_accounts)),
			to_accounts: Arc::from(RwLock::from(to_accounts)),
			nonces: Default::default(),
			rpc_client: RpcClient::from_url(uri).await.expect(EXPECT_CONNECT),
			current_pending_extrinsics: None.into(),
		}
	}

	fn api(&self) -> OnlineClient<C> {
		self.api.clone()
	}

	pub fn get_from_account_id(&self, account: &String) -> Option<AccountIdOf<C>> {
		self.from_accounts.read().get(account).map(|a| a.0.account_id())
	}

	fn get_to_account_id(&self, account: &String) -> Option<AccountIdOf<C>> {
		self.to_accounts.read().get(account).map(|a| a.0.account_id())
	}

	fn get_to_account_metadata(&self, account: &String) -> Option<AccountMetadata> {
		self.to_accounts.read().get(account).map(|a| a.1.clone())
	}

	fn get_from_key_pair(&self, account: &String) -> Option<KP> {
		self.from_accounts.read().get(account).map(|k| k.0.clone())
	}

	pub async fn check_account_nonce(
		&self,
		account: AccountIdOf<C>,
	) -> Result<u128, Box<dyn std::error::Error>> {
		if let Some(nonce) = self.nonces.write().get_mut(&hex::encode(account.clone())) {
			*nonce = *nonce + 1;
			return Ok(*nonce)
		}
		{
			let nonce = check_account_nonce(self.api.clone(), account.clone()).await?;
			self.nonces.write().insert(hex::encode(account), nonce);
			Ok(nonce)
		}
	}

	async fn update_count(&self) {
		let i = Instant::now();
		let xts = self
			.rpc_client
			.request::<Vec<serde_json::Value>>("author_pendingExtrinsics", rpc_params![])
			.await
			.expect("author_pendingExtrinsics should not fail");
		*self.current_pending_extrinsics.write() = Some((i, xts.len()));
	}
}

pub async fn check_account_nonce<C: subxt::Config>(
	api: OnlineClient<C>,
	account: AccountIdOf<C>,
) -> Result<u128, Box<dyn std::error::Error>>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
{
	let storage_query =
		subxt::dynamic::storage("System", "Account", vec![Value::from_bytes(account.clone())]);
	let result = api.storage().at_latest().await?.fetch(&storage_query).await?;
	let value = result
		.ok_or(format!("Sender account {:?} does not exist", hex::encode(account.clone())))?
		.to_value()?;

	debug!(target:LOG_TARGET,"account has free balance: {:?}", value.at("data").at("free"));
	debug!(target:LOG_TARGET,"account has nonce: {:?}", value.at("nonce"));
	// info!("account has nonce: {:#?}", value);
	let nonce = value
		.at("nonce")
		.ok_or("nonce is not set for the account")?
		.as_u128()
		.ok_or("nonce is not u128")?;

	Ok(nonce)
}

#[async_trait]
impl<C, KP> TransactionsSink<<C as subxt::Config>::Hash> for SubxtTransactionsSink<C, KP>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
	C: subxt::Config,
	KP: Signer<C> + Clone + Send + Sync + 'static,
{
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
	) -> Result<StreamOf<TransactionStatus<<C as subxt::Config>::Hash>>, Error> {
		let tx = tx.as_any().downcast_ref::<SubxtTransaction<C>>().unwrap();
		let result = tx.extrinsic.submit_and_watch().await;

		match result {
			Ok(stream) => Ok(stream
				.map(|e| {
					// info!(evnt=?e, "SubxtTransactionsSink::map");
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
	async fn count(&self) -> usize {
		let current_pending_extrinsics = { *self.current_pending_extrinsics.read() };
		if let Some((ts, _)) = current_pending_extrinsics {
			if ts.elapsed() > Duration::from_millis(1000) {
				self.update_count().await;
			}
		} else {
			self.update_count().await;
		}

		self.current_pending_extrinsics
			.read()
			.expect("current_pending_extrinsics cannot be None")
			.1
	}
}

#[derive(Debug, Clone)]
pub enum AccountGenerateRequest {
	Keyring(String),
	Derived(String, u32),
}

const SENDER_SEED: &str = "//Sender";
const RECEIVER_SEED: &str = "//Receiver";
const SEED: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

pub fn generate_ecdsa_keypair(description: AccountGenerateRequest) -> EthKeypair {
	match description {
		AccountGenerateRequest::Keyring(name) => match name.as_str() {
			"alice" | "alith" => eth_dev::alith(),
			"bob" | "baltathar" => eth_dev::baltathar(),
			"charlie" | "charleth" => eth_dev::charleth(),
			"dave" | "dorothy" => eth_dev::dorothy(),
			"eve" | "ethan" => eth_dev::ethan(),
			"ferdie" | "faith" => eth_dev::faith(),
			_ => panic!("unknown keyring name"),
		},
		AccountGenerateRequest::Derived(seed, i) => {
			use std::str::FromStr;
			let derivation = format!("{SEED}{seed}//{i}");
			let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
			<subxt_signer::ecdsa::Keypair>::from_uri(&u).unwrap().into()
		},
	}
}
pub fn generate_sr25519_keypair(description: AccountGenerateRequest) -> SrPair {
	match description {
		AccountGenerateRequest::Keyring(name) => match name.as_str() {
			"alice" | "alith" => sr25519_dev::alice(),
			"bob" | "baltathar" => sr25519_dev::bob(),
			"charlie" | "charleth" => sr25519_dev::charlie(),
			"dave" | "dorothy" => sr25519_dev::dave(),
			"eve" | "ethan" => sr25519_dev::eve(),
			"ferdie" | "faith" => sr25519_dev::ferdie(),
			_ => panic!("unknown keyring name"),
		},
		AccountGenerateRequest::Derived(seed, i) => {
			use std::str::FromStr;
			let derivation = format!("{SEED}{seed}/{i}");
			let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
			<subxt_signer::sr25519::Keypair>::from_uri(&u).unwrap().into()
		},
	}
}

pub trait GenerateKeyPairFunction<KP>:
	Fn(AccountGenerateRequest) -> KP + Copy + Send + 'static
{
}
impl<T, KP> GenerateKeyPairFunction<KP> for T where
	T: Fn(AccountGenerateRequest) -> KP + Copy + Send + 'static
{
}

pub fn derive_accounts<C, KP, G>(
	accounts_description: AccountsDescription,
	seed: &str,
	generate: G,
) -> HashMap<String, (KP, AccountMetadata)>
where
	C: subxt::Config,
	KP: Signer<C> + Send + Sync + 'static,
	G: GenerateKeyPairFunction<KP>,
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
							(
								i.to_string(),
								(
									generate(AccountGenerateRequest::Derived(
										seed.to_string(),
										i as u32,
									)),
									AccountMetadata::Derived(i as u32),
								),
							)
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
		AccountsDescription::Keyring(account) => HashMap::from([(
			account.clone(),
			(
				generate(AccountGenerateRequest::Keyring(account.clone())),
				AccountMetadata::KeyRing(account),
			),
		)]),
	}
}

pub trait GenerateTxPayloadFunction<A: Send + Sync + AsRef<[u8]>>:
	Fn(A) -> DynamicPayload + Copy + Send + 'static
{
}

impl<T, A: Send + Sync + AsRef<[u8]>> GenerateTxPayloadFunction<A> for T where
	T: Fn(A) -> DynamicPayload + Copy + Send + 'static
{
}

pub fn build_substrate_tx_payload(to_account_id: AccountIdOf<PolkadotConfig>) -> DynamicPayload {
	trace!(target:LOG_TARGET,to_account=hex::encode(to_account_id.clone()),"build_payload (sub)" );
	subxt::dynamic::tx(
		"Balances",
		"transfer_keep_alive",
		vec![
			Value::unnamed_variant("Id", [Value::from_bytes(to_account_id)]),
			Value::u128(1u32.into()),
		],
	)
}

pub fn build_eth_tx_payload(to_account_id: AccountId20) -> DynamicPayload {
	trace!(target:LOG_TARGET,to_account=hex::encode(to_account_id.clone()),"build_payload (eth)");
	subxt::dynamic::tx(
		"Balances",
		"transfer_keep_alive",
		vec![
			Value::unnamed_composite(vec![Value::from_bytes(to_account_id)]),
			Value::u128(1u32.into()),
		],
	)
}

pub async fn build_subxt_tx<C, KP, G>(
	account: &String,
	nonce: &Option<u128>,
	sink: &SubxtTransactionsSink<C, KP>,
	generate_payload: G,
) -> SubxtTransaction<C>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
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
	G: GenerateTxPayloadFunction<AccountIdOf<C>>,
{
	let to_account_id = sink.get_to_account_id(account).expect("to account exists");
	let from_account_id = sink.get_from_account_id(account).expect("from account exists");
	let from_keypair = sink.get_from_key_pair(account).expect("from account exists");
	let nonce = if let Some(nonce) = nonce {
		trace!("nonce for {:?} -> {:?}", account, nonce);
		*nonce
	} else {
		let nonce = sink
			.check_account_nonce(from_account_id.clone())
			.await
			.expect("account nonce shall exists");
		trace!("checked nonce for {:?} -> {:?}", account, nonce);
		nonce
	};
	debug!(
		target:LOG_TARGET,
		account,
		nonce,
		from_account=hex::encode(from_account_id),
		to_account=hex::encode(to_account_id.clone()),
		"build_subxt_tx"
	);

	let tx_params = <SubstrateExtrinsicParamsBuilder<C>>::new().nonce(nonce as u64).build().into();
	let tx_call = generate_payload(to_account_id);

	let tx = SubxtTransaction::<C>::new(
		sink.api()
			.tx()
			.create_signed_offline(&tx_call, &from_keypair, tx_params)
			.unwrap(),
		nonce as u128,
		sink.get_to_account_metadata(account).expect("account metadata exists"),
	);

	debug!(target:LOG_TARGET,"built tx hash: {:?}", tx.hash());

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
	async fn placeholder() -> Result<(), Box<dyn std::error::Error>> {
		//todo add tests....
	}
}
