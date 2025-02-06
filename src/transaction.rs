use crate::error::Error;
use async_trait::async_trait;
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::{any::Any, ops::Range, pin::Pin};
use subxt::{config::BlockHash, tx::TxStatus, OnlineClient};

/// What account was used to sign transaction
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub enum AccountMetadata {
	/// Holds index used for in account derivation
	#[default]
	None,
	Derived(u32),
	KeyRing(String),
}

#[derive(Clone)]
pub enum TransactionCall {
	Transfer,
	Remark(u32),
}

#[derive(Clone)]
pub struct TransactionRecipe {
	pub call: TransactionCall,
}

impl TransactionRecipe {
	pub fn transfer() -> Self {
		Self { call: TransactionCall::Transfer }
	}

	pub fn remark(size: u32) -> Self {
		Self { call: TransactionCall::Remark(size) }
	}
}

pub trait TransactionStatusIsTerminal {
	fn is_terminal(&self) -> bool;
	fn is_finalized(&self) -> bool;
	fn is_error(&self) -> bool;
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub enum TransactionStatus<H> {
	Validated,
	Broadcasted,
	InBlock(H),
	NoLongerInBestBlock,
	Finalized(H),
	Dropped(String),
	Invalid(String),
	Error(String),
}

impl<H> TransactionStatus<H> {
	pub fn get_letter(&self) -> char {
		match self {
			TransactionStatus::Validated => 'V',
			TransactionStatus::Broadcasted { .. } => 'b',
			TransactionStatus::InBlock(..) => 'B',
			TransactionStatus::Finalized(..) => 'F',
			TransactionStatus::Error { .. } => 'E',
			TransactionStatus::Invalid { .. } => 'I',
			TransactionStatus::Dropped { .. } => 'D',
			TransactionStatus::NoLongerInBestBlock => 'N',
		}
	}
}

impl<H: BlockHash + std::fmt::Debug> TransactionStatus<H> {}

impl<C: subxt::Config> From<TxStatus<C, OnlineClient<C>>> for TransactionStatus<C::Hash> {
	fn from(value: TxStatus<C, OnlineClient<C>>) -> Self {
		match value {
			TxStatus::Validated => TransactionStatus::Validated,
			TxStatus::Broadcasted => TransactionStatus::Broadcasted,
			TxStatus::InBestBlock(tx) => TransactionStatus::InBlock(tx.block_hash()),
			TxStatus::InFinalizedBlock(tx) => TransactionStatus::Finalized(tx.block_hash()),
			TxStatus::Error { message } => TransactionStatus::Error(message),
			TxStatus::Invalid { message } => TransactionStatus::Invalid(message),
			TxStatus::Dropped { message } => TransactionStatus::Dropped(message),
			TxStatus::NoLongerInBestBlock => TransactionStatus::NoLongerInBestBlock,
		}
	}
}

impl<H: BlockHash> TransactionStatusIsTerminal for TransactionStatus<H> {
	fn is_terminal(&self) -> bool {
		matches!(self, Self::Finalized(_) | Self::Dropped(_) | Self::Invalid(_) | Self::Error(_))
	}

	fn is_finalized(&self) -> bool {
		matches!(self, Self::Finalized(_))
	}

	fn is_error(&self) -> bool {
		matches!(self, Self::Dropped(_) | Self::Invalid(_) | Self::Error(_))
	}
}

pub trait Transaction: Send + Sync {
	type HashType: BlockHash + 'static;
	fn hash(&self) -> Self::HashType;
	fn as_any(&self) -> &dyn Any;
	fn nonce(&self) -> u128;
	fn account_metadata(&self) -> AccountMetadata;
}

pub trait ResubmitHandler: Sized {
	fn handle_resubmit_request(self) -> Option<Self>;
}

#[async_trait]
pub trait TransactionMonitor<H: BlockHash> {
	async fn wait(&self, tx_hash: H) -> H;
}

pub type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

/// Abstraction for RPC client
#[async_trait]
pub trait TransactionsSink<H: BlockHash>: Send + Sync {
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = H>,
	) -> Result<StreamOf<TransactionStatus<H>>, Error>;

	async fn submit(&self, tx: &dyn Transaction<HashType = H>) -> Result<H, Error>;

	///Current count of transactions being processed by sink
	async fn count(&self) -> usize;

	fn transaction_monitor(&self) -> Option<&dyn TransactionMonitor<H>>;
}

#[derive(Subcommand)]
/// Send transactions to the node using different scenarios.
pub enum SendingScenario {
	/// Send single transactions to the node.
	OneShot {
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or number of
		/// pre-funded account, index used for derivation.
		#[clap(long, default_value = "alice")]
		account: String,
		/// Nonce used for the account.
		#[clap(long)]
		nonce: Option<u128>,
	},
	/// Send multiple transactions to the node using a single account.
	FromSingleAccount {
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or number of
		/// pre-funded account, index used for derivation.
		#[clap(long, default_value = "alice")]
		account: String,
		/// Starting nonce for 1st transaction in the batch. If not given the current nonce for
		/// the account will be fetched from node for the first transaction in the batch.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch.
		#[clap(long, default_value_t = 1)]
		count: u32,
	},
	/// Send multiple transactions to the node using multiple accounts.
	FromManyAccounts {
		/// First account identifier to be used (index of the pre-funded account used for a
		/// derivation).
		#[clap(long)]
		start_id: u32,
		/// Last account identifier to be used.
		#[clap(long)]
		last_id: u32,
		/// Starting nonce of transactions batch. If not given the current nonce for each account
		/// will be fetched from node.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch per account.
		#[clap(long, default_value_t = 1)]
		count: u32,
	},
}

#[derive(Debug, Clone)]
pub enum AccountsDescription {
	Keyring(String),
	Derived(Range<u32>),
}

impl SendingScenario {
	pub fn get_accounts_description(&self) -> AccountsDescription {
		match self {
			Self::OneShot { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
			Self::FromManyAccounts { start_id, last_id, .. } =>
				AccountsDescription::Derived(*start_id..last_id + 1),
			Self::FromSingleAccount { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
		}
	}
}

#[derive(ValueEnum, Clone)]
pub enum ChainType {
	/// Substrate compatible chain.
	Sub,
	/// Etheruem compatible chain.
	Eth,
	/// Do not send transactions anywhere, just for dev/testing.
	Fake,
}
