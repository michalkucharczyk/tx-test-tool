use crate::error::Error;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{any::Any, pin::Pin};
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

pub trait TransactionStatusIsTerminal {
	fn is_terminal(&self) -> bool;
	fn is_finalized(&self) -> bool;
	fn is_error(&self) -> bool;
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub enum TransactionStatus<H> {
	Validated,
	Broadcasted(u32),
	InBlock(H),
	NoLongerInBestBlock,
	Finalized(H),
	Dropped(String),
	Invalid(String),
	Error(String),
}

impl<H: BlockHash + std::fmt::Debug> TransactionStatus<H> {}

impl<C: subxt::Config> From<TxStatus<C, OnlineClient<C>>> for TransactionStatus<C::Hash> {
	fn from(value: TxStatus<C, OnlineClient<C>>) -> Self {
		match value {
			TxStatus::Validated => TransactionStatus::Validated,
			TxStatus::Broadcasted { num_peers } => TransactionStatus::Broadcasted(num_peers),
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
