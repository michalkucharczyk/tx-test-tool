use crate::{
	block_monitor::BlockMonitor,
	error::Error,
	helpers::StreamOf,
	scenario::AccountsDescription,
	transaction::{
		AccountMetadata, Transaction, TransactionCall, TransactionMonitor, TransactionRecipe,
		TransactionStatus, TransactionsSink,
	},
};
use async_trait::async_trait;
use futures::StreamExt;
use parking_lot::RwLock;
use std::{
	any::Any,
	collections::HashMap,
	sync::Arc,
	time::{Duration, Instant},
};
use subxt::{
	backend::rpc::RpcClient,
	config::{
		transaction_extensions::{
			ChargeAssetTxPaymentParams, ChargeTransactionPaymentParams, CheckMortalityParams,
			CheckNonceParams,
		},
		DefaultExtrinsicParams, ExtrinsicParams,
	},
	dynamic::{At, Value},
	ext::scale_value::value,
	tx::{DynamicPayload, Signer, SubmittableTransaction},
	OnlineClient, PolkadotConfig,
};
use subxt_core::{config::SubstrateExtrinsicParamsBuilder, utils::AccountId20};
use subxt_signer::{
	eth::{dev as eth_dev, Keypair as EthKeypair, Signature},
	sr25519::{dev as sr25519_dev, Keypair as SrPair},
};
use tracing::{debug, error, trace};

const LOG_TARGET: &str = "subxt_tx";
const DEFAULT_RETRIES_FOR_PARTIAL_TX_CREATION: usize = 10;

#[derive(Clone)]
/// Ethereum runtime config definition for subxt usage purposes.
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

/// Type alias for subxt config hash.
pub(crate) type HashOf<C> = <C as subxt::Config>::Hash;
/// Type alias for subxt account id.
pub(crate) type AccountIdOf<C> = <C as subxt::Config>::AccountId;

/// A subxt transaction abstraction.
#[derive(Clone)]
pub struct SubxtTransaction<C: subxt::Config> {
	transaction: Arc<SubmittableTransaction<C, OnlineClient<C>>>,
	nonce: u128,
	valid_until: Option<u64>,
	account_metadata: AccountMetadata,
}

/// Transaction type thart runs on `Ethereum` compatible chains.
pub type EthTransaction = SubxtTransaction<EthRuntimeConfig>;
/// Holds the RPC API connection for transaction execution.
pub type EthTransactionsSink = SubxtTransactionsSink<EthRuntimeConfig, EthKeypair>;
/// Transaction type that runs on `substrate` compatible chains.
pub type SubstrateTransaction = SubxtTransaction<PolkadotConfig>;
/// Holds the RPC API connection for transaction execution.
pub type SubstrateTransactionsSink = SubxtTransactionsSink<PolkadotConfig, SrPair>;

impl<C: subxt::Config> SubxtTransaction<C> {
	pub fn new(
		transaction: SubmittableTransaction<C, OnlineClient<C>>,
		nonce: u128,
		valid_until: Option<u64>,
		account_metadata: AccountMetadata,
	) -> Self {
		Self { transaction: Arc::new(transaction), nonce, account_metadata, valid_until }
	}
}

// type TransactionSubxt2 = subxt::tx::DynamicPayload;

impl<C: subxt::Config> Transaction for SubxtTransaction<C> {
	type HashType = <C as subxt::Config>::Hash;
	fn hash(&self) -> Self::HashType {
		self.transaction.hash()
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
	fn valid_until(&self) -> &Option<u64> {
		&self.valid_until
	}
}

#[derive(Clone)]
pub struct SubxtTransactionsSink<C: subxt::Config, KP: Signer<C>> {
	api: OnlineClient<C>,
	from_accounts: Arc<RwLock<HashMap<String, (KP, AccountMetadata)>>>,
	to_accounts: Arc<RwLock<HashMap<String, (KP, AccountMetadata)>>>,
	nonces: Arc<RwLock<HashMap<String, u128>>>,
	rpc_client: RpcClient,
	current_pending_extrinsics: Arc<RwLock<Option<(Instant, usize)>>>,
	block_monitor: Option<BlockMonitor<C>>,
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
			api: crate::subxt_api_connector::connect("ws://127.0.0.1:9933", false)
				.await
				.expect(EXPECT_CONNECT),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
			rpc_client: RpcClient::from_url("ws://127.0.0.1:9933").await.expect(EXPECT_CONNECT),
			current_pending_extrinsics: Arc::new(None.into()),
			block_monitor: None,
		}
	}

	pub async fn new_with_uri(uri: &String) -> Self {
		Self {
			api: crate::subxt_api_connector::connect(uri, false).await.expect(EXPECT_CONNECT),
			from_accounts: Default::default(),
			to_accounts: Default::default(),
			nonces: Default::default(),
			rpc_client: RpcClient::from_url(uri).await.expect(EXPECT_CONNECT),
			current_pending_extrinsics: Arc::new(None.into()),
			block_monitor: None,
		}
	}

	pub async fn new_with_uri_with_accounts_description<G>(
		uri: &str,
		accounts_description: AccountsDescription,
		generate_pair: G,
		block_monitor: Option<BlockMonitor<C>>,
		use_legacy_backend: bool,
	) -> Self
	where
		G: GenerateKeyPairFunction<KP>,
	{
		let from_accounts =
			derive_accounts(accounts_description.clone(), SENDER_SEED, generate_pair);
		let to_accounts = derive_accounts(accounts_description, RECEIVER_SEED, generate_pair);
		Self {
			api: crate::subxt_api_connector::connect(uri, use_legacy_backend)
				.await
				.expect(EXPECT_CONNECT),
			from_accounts: Arc::from(RwLock::from(from_accounts)),
			to_accounts: Arc::from(RwLock::from(to_accounts)),
			nonces: Default::default(),
			rpc_client: crate::helpers::client(uri).await.expect(EXPECT_CONNECT).into(),
			current_pending_extrinsics: Arc::new(None.into()),
			block_monitor,
		}
	}

	fn api(&self) -> OnlineClient<C> {
		self.api.clone()
	}

	pub fn get_from_account_id(&self, account: &str) -> Option<AccountIdOf<C>> {
		self.from_accounts.read().get(account).map(|a| a.0.account_id())
	}

	fn get_to_account_id(&self, account: &str) -> Option<AccountIdOf<C>> {
		self.to_accounts.read().get(account).map(|a| a.0.account_id())
	}

	fn get_to_account_metadata(&self, account: &str) -> Option<AccountMetadata> {
		self.to_accounts.read().get(account).map(|a| a.1.clone())
	}

	fn get_from_key_pair(&self, account: &str) -> Option<KP> {
		self.from_accounts.read().get(account).map(|k| k.0.clone())
	}

	pub async fn check_account_nonce(
		&self,
		account: AccountIdOf<C>,
	) -> Result<u128, Box<dyn std::error::Error>> {
		let is_nonce_set = {
			let nonces = self.nonces.read();
			nonces.get(&hex::encode(account.clone())).cloned()
		};

		let remote_nonce = if let Some(nonce) = is_nonce_set {
			nonce
		} else {
			check_account_nonce(self.api.clone(), account.clone()).await?
		};

		let mut nonces = self.nonces.write();
		if let Some(nonce) = nonces.get_mut(&hex::encode(account.clone())) {
			*nonce += 1;
			Ok(*nonce)
		} else {
			nonces.insert(hex::encode(account), remote_nonce);
			Ok(remote_nonce)
		}
	}

	async fn update_count(&self) {
		let i = Instant::now();
		let xts_len = self
			.rpc_client
			.request::<Vec<serde_json::Value>>(
				"author_pendingExtrinsics",
				subxt_rpcs::rpc_params!(),
			)
			.await
			.expect("author_pendingExtrinsics should not fail")
			.len();
		*self.current_pending_extrinsics.write() = Some((i, xts_len));
	}
}

/// Fetches an account storage and returns its nonce.
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
		let result = tx.transaction.submit_and_watch().await;

		match result {
			Ok(stream) => Ok(stream.map(|e| e.unwrap().into()).boxed()),
			Err(e) => Err(e.into()),
		}
	}

	async fn submit(
		&self,
		tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
	) -> Result<<C as subxt::Config>::Hash, Error> {
		let tx = tx.as_any().downcast_ref::<SubxtTransaction<C>>().unwrap();
		tx.transaction.submit().await.map_err(|e| e.into())
	}

	/// Current count of transactions being processed by sink.
	async fn pending_extrinsics(&self) -> usize {
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

	fn transaction_monitor(&self) -> Option<&dyn TransactionMonitor<<C as subxt::Config>::Hash>> {
		self.block_monitor
			.as_ref()
			.map(|m| m as &dyn TransactionMonitor<<C as subxt::Config>::Hash>)
	}
}

/// Types of accounts generation.
#[derive(Debug, Clone)]
pub enum AccountGenerateRequest {
	Keyring(String),
	Derived(String, u32),
}

/// Seed user for sender accounts.
pub const SENDER_SEED: &str = "//Sender";
/// Seed used for receiver accounts.
pub(crate) const RECEIVER_SEED: &str = "//Receiver";

/// Generates ecdsa based keypairs.
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
			let derivation = format!("{seed}//{i}");
			let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
			<subxt_signer::ecdsa::Keypair>::from_uri(&u).unwrap().into()
		},
	}
}

/// Generates sr25519 based keypairs.
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
			let derivation = format!("{seed}//{i}");
			let u = subxt_signer::SecretUri::from_str(&derivation).unwrap();
			<subxt_signer::sr25519::Keypair>::from_uri(&u).unwrap()
		},
	}
}

/// Interface for implementors of keypairs generators.
pub trait GenerateKeyPairFunction<KP>:
	Fn(AccountGenerateRequest) -> KP + Copy + Send + 'static
{
}

impl<T, KP> GenerateKeyPairFunction<KP> for T where
	T: Fn(AccountGenerateRequest) -> KP + Copy + Send + 'static
{
}

/// Logic that derives accounts from a certain seed.
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

			(0..t).for_each(|thread_idx| {
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
				.flat_map(|h| h.join().unwrap())
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
	Fn(A, &TransactionRecipe) -> DynamicPayload + Copy + Send + 'static
{
}

impl<T, A: Send + Sync + AsRef<[u8]>> GenerateTxPayloadFunction<A> for T where
	T: Fn(A, &TransactionRecipe) -> DynamicPayload + Copy + Send + 'static
{
}

/// Generates a transaction payload given a signer account and a transaction recipe.
pub(crate) fn build_substrate_tx_payload(
	to_account_id: AccountIdOf<PolkadotConfig>,
	recipe: &TransactionRecipe,
) -> DynamicPayload {
	trace!(target:LOG_TARGET,to_account=hex::encode(to_account_id.clone()),"build_payload (sub)" );

	match recipe.call {
		TransactionCall::Remark(s) => {
			let i = hex::encode(to_account_id.clone()).as_bytes().last().copied().unwrap();
			let data = vec![i; s as usize * 1024];
			subxt::dynamic::tx("System", "remark", vec![data])
		},
		TransactionCall::Transfer => {
			//works for rococo:
			subxt::dynamic::tx(
				"Balances",
				"transfer_keep_alive",
				vec![value!(Id(Value::from_bytes(to_account_id))), Value::u128(1u32.into())],
			)
		},
	}
}

/// Crates a raw eth transaction.
pub(crate) fn build_eth_tx_payload(
	to_account_id: AccountId20,
	recipe: &TransactionRecipe,
) -> DynamicPayload {
	trace!(target:LOG_TARGET,to_account=hex::encode(to_account_id),"build_payload (eth)");
	match recipe.call {
		TransactionCall::Remark(s) => {
			let i = hex::encode(to_account_id).as_bytes().last().copied().unwrap();
			let data = vec![i; s as usize];
			subxt::dynamic::tx("System", "remark", vec![data])
		},
		TransactionCall::Transfer => subxt::dynamic::tx(
			"Balances",
			"transfer_keep_alive",
			vec![
				Value::unnamed_composite(vec![Value::from_bytes(to_account_id)]),
				Value::u128(1u32.into()),
			],
		),
	}
}

async fn create_transaction<C: subxt::Config, KP, G>(
	from_keypair: &KP,
	nonce: u128,
	mortality: &Option<u64>,
	account: &str,
	sink: &SubxtTransactionsSink<C, KP>,
	from_account_id: &<C as subxt::Config>::AccountId,
	to_account_id: &<C as subxt::Config>::AccountId,
	recipe: &TransactionRecipe,
	generate_payload: G,
) -> Result<SubxtTransaction<C>, Error>
where
	G: GenerateTxPayloadFunction<AccountIdOf<C>>,
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
	KP: Signer<C> + Clone + Send + Sync + 'static,
	<<C as subxt::Config>::ExtrinsicParams as subxt::config::ExtrinsicParams<C>>::Params: From<(
		(),
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
	// Needed because `Params` as associated type does not implement clone, and we need to
	// recreate the tx params in a loop when we can't create a partial tx with the online
	// client, due to various RPC related issues or state not being up to date (currently we
	// handle an error which happens when trying to create a partial tx that is based on a
	// certain finalized block returned by the RPC, which is then reported as not found).
	// Retrying seems to fix the issue.
	fn tx_params<CC: subxt::Config>(
		mortality: &Option<u64>,
		nonce: u64,
		recipe: &TransactionRecipe,
	) -> <DefaultExtrinsicParams<CC> as ExtrinsicParams<CC>>::Params {
		let mut params = <SubstrateExtrinsicParamsBuilder<CC>>::new().nonce(nonce).tip(recipe.tip);
		if let Some(mortal) = mortality {
			params = params.mortal(*mortal);
		}
		params.build()
	}

	let params = tx_params(mortality, nonce as u64, recipe);
	let tx_call = generate_payload(to_account_id.clone(), recipe);
	match sink.api().tx().create_partial(&tx_call, from_account_id, params.into()).await {
		Ok(mut tx) => {
			let block_ref = sink
				.api()
				.backend()
				.latest_finalized_block_ref()
				.await
				.expect("to get the last finalized block ref. qed");
			let block = sink
				.api()
				.blocks()
				.at(block_ref)
				.await
				.expect("to get the corresponding block header. qed");
			let submittable_tx = tx.sign(from_keypair);
			let hash = submittable_tx.hash();
			let tx = SubxtTransaction::<C>::new(
				submittable_tx,
				nonce,
				mortality.map(|mortal| block.number().into() + mortal),
				sink.get_to_account_metadata(account).expect("account metadata exists"),
			);
			debug!(target:LOG_TARGET,"built mortal tx hash: {:?}", hash);
			Ok(tx)
		},
		Err(err) => match err {
			subxt::Error::Block(subxt::error::BlockError::NotFound(_)) => {
				let tx_call = generate_payload(to_account_id.clone(), recipe);
				for _ in 0..DEFAULT_RETRIES_FOR_PARTIAL_TX_CREATION {
					let params = tx_params(mortality, nonce as u64, recipe);
					match sink
						.api()
						.tx()
						.create_partial(&tx_call, from_account_id, params.into())
						.await
					{
						Ok(mut tx) => {
							let block_ref = sink
								.api()
								.backend()
								.latest_finalized_block_ref()
								.await
								.expect("to get the last finalized block ref. qed");
							let block = sink
								.api()
								.blocks()
								.at(block_ref)
								.await
								.expect("to get the corresponding block header. qed");
							let submittable_tx = tx.sign(from_keypair);
							let hash = submittable_tx.hash();
							let subxt_tx = SubxtTransaction::<C>::new(
								submittable_tx,
								nonce,
								mortality.map(|mortal| block.number().into() + mortal),
								sink.get_to_account_metadata(account)
									.expect("account metadata exists"),
							);
							debug!(target:LOG_TARGET,"built mortal tx hash: {:?}", hash);
							return Ok(subxt_tx)
						},
						Err(_) => continue,
					}
				}
				Err(Error::Other(format!("Creating transaction after {DEFAULT_RETRIES_FOR_PARTIAL_TX_CREATION} iterations failed. Check logs for more info!")))
			},
			err => {
				error!(
					target: LOG_TARGET,
					account,
					nonce,
					?mortality,
					from_account=hex::encode(from_account_id.clone()),
					to_account=hex::encode(to_account_id.clone()),
					%err,
					"build_subxt_tx: create partial tx failure"
				);
				Err(Error::Other(format!("can not create transaction due to: {}", err)))
			},
		},
	}
}

/// Builds a transaction with subxt.
pub(crate) async fn build_subxt_tx<C, KP, G>(
	account: &str,
	nonce: &Option<u128>,
	mortality: &Option<u64>,
	sink: &SubxtTransactionsSink<C, KP>,
	recipe: &TransactionRecipe,
	generate_payload: G,
) -> SubxtTransaction<C>
where
	AccountIdOf<C>: Send + Sync + AsRef<[u8]>,
	C: subxt::Config,
	KP: Signer<C> + Clone + Send + Sync + 'static,
	<<C as subxt::Config>::ExtrinsicParams as subxt::config::ExtrinsicParams<C>>::Params: From<(
		(),
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
		?mortality,
		from_account=hex::encode(from_account_id.clone()),
		to_account=hex::encode(to_account_id.clone()),
		"build_subxt_tx"
	);

	if let Ok(tx) = create_transaction(
		&from_keypair,
		nonce,
		mortality,
		account,
		sink,
		&from_account_id,
		&to_account_id,
		&recipe,
		generate_payload,
	)
	.await
	{
		tx
	} else {
		let params = <SubstrateExtrinsicParamsBuilder<C>>::new()
			.nonce(nonce as u64)
			.tip(recipe.tip)
			.build();
		let tx_call = generate_payload(to_account_id, recipe);
		let tx = SubxtTransaction::<C>::new(
			sink.api()
				.tx()
				.create_partial_offline(&tx_call, params.into())
				.unwrap()
				.sign(&from_keypair),
			nonce as u128,
			None,
			sink.get_to_account_metadata(account).expect("account metadata exists"),
		);
		debug!(target:LOG_TARGET,"built immortal tx hash: {:?}", tx.hash());
		tx
	}

	// } else {
	// 	}
}

#[cfg(test)]
mod tests {
	use subxt::SubstrateConfig;

	use crate::{
		subxt_transaction::{
			derive_accounts, generate_sr25519_keypair, AccountGenerateRequest, SENDER_SEED,
		},
		transaction::AccountMetadata,
	};

	#[tokio::test]
	async fn test_derive_accounts_len() {
		let accounts = derive_accounts::<SubstrateConfig, subxt_signer::sr25519::Keypair, _>(
			crate::scenario::AccountsDescription::Derived(0..11),
			SENDER_SEED,
			generate_sr25519_keypair,
		);
		assert_eq!(accounts.len(), 11);
		for (i, (kp, meta)) in accounts {
			let id = i.parse::<u32>().unwrap();
			assert_eq!(
				kp.public_key().0,
				generate_sr25519_keypair(AccountGenerateRequest::Derived(
					SENDER_SEED.to_string(),
					id
				))
				.public_key()
				.0
			);
			assert_eq!(AccountMetadata::Derived(id), meta);
		}

		let accounts = derive_accounts::<SubstrateConfig, subxt_signer::sr25519::Keypair, _>(
			crate::scenario::AccountsDescription::Keyring("alice".to_string()),
			SENDER_SEED,
			generate_sr25519_keypair,
		);
		assert_eq!(accounts.len(), 1);
		assert_eq!(
			accounts.get("alice").unwrap().0.public_key().0,
			generate_sr25519_keypair(AccountGenerateRequest::Keyring("alice".to_string()))
				.public_key()
				.0
		);
		assert_eq!(accounts.get("alice").unwrap().1, AccountMetadata::KeyRing("alice".to_string()))
	}
}
