// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use crate::{
	error::Error,
	fake_transaction::{FakeHash, FakeTransaction},
	fake_transaction_sink::FakeTransactionsSink,
	helpers::StreamOf,
	runner::DefaultTxTask,
	subxt_transaction::{
		build_subxt_tx, EthPayloadBuilderFn, EthRuntimeConfig, EthTransaction, EthTransactionsSink,
		HashOf, SubPayloadBuilderFn, SubstrateTransaction, SubstrateTransactionsSink,
	},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::any::Any;
use subxt::{config::BlockHash, tx::TxStatus, OnlineClient, PolkadotConfig};

/// Parameters for building a transaction.
pub(crate) struct BuildTransactionParams<'a> {
	pub account: &'a str,
	pub nonce: &'a Option<u128>,
	pub mortality: &'a Option<u64>,
	pub tip: u128,
}

/// Interface for transaction building.
#[async_trait]
pub(crate) trait TransactionBuilder {
	type HashType: BlockHash;
	type Transaction: Transaction<HashType = Self::HashType>;
	type Sink: TransactionsSink<Self::HashType>;
	type PayloadBuilder: Send + Sync;

	async fn build_transaction<'a>(
		&self,
		watched: bool,
		params: BuildTransactionParams<'a>,
		sink: &Self::Sink,
		payload_builder: &Self::PayloadBuilder,
	) -> DefaultTxTask<Self::Transaction>;
}

/// Substrate transactions builder.
#[derive(Default)]
pub(crate) struct SubstrateTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for SubstrateTransactionBuilder {
	type HashType = HashOf<PolkadotConfig>;
	type Transaction = SubstrateTransaction;
	type Sink = SubstrateTransactionsSink;
	type PayloadBuilder = SubPayloadBuilderFn;

	async fn build_transaction<'a>(
		&self,
		watched: bool,
		params: BuildTransactionParams<'a>,
		sink: &Self::Sink,
		payload_builder: &Self::PayloadBuilder,
	) -> DefaultTxTask<Self::Transaction> {
		let tx = build_subxt_tx(&params, sink, &**payload_builder).await;
		if watched {
			DefaultTxTask::<Self::Transaction>::new_watched(tx)
		} else {
			DefaultTxTask::<Self::Transaction>::new_unwatched(tx)
		}
	}
}

/// Ethereum transactions builder.
#[derive(Default)]
pub(crate) struct EthTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for EthTransactionBuilder {
	type HashType = HashOf<EthRuntimeConfig>;
	type Transaction = EthTransaction;
	type Sink = EthTransactionsSink;
	type PayloadBuilder = EthPayloadBuilderFn;

	async fn build_transaction<'a>(
		&self,
		watched: bool,
		params: BuildTransactionParams<'a>,
		sink: &Self::Sink,
		payload_builder: &Self::PayloadBuilder,
	) -> DefaultTxTask<Self::Transaction> {
		let tx = build_subxt_tx(&params, sink, &**payload_builder).await;
		if watched {
			DefaultTxTask::<Self::Transaction>::new_watched(tx)
		} else {
			DefaultTxTask::<Self::Transaction>::new_unwatched(tx)
		}
	}
}

#[allow(dead_code)]
#[derive(Default)]
/// A transaction builder sink that's used as mock for logic relying on a transaction builder.
pub(crate) struct FakeTransactionBuilder;

#[async_trait]
impl TransactionBuilder for FakeTransactionBuilder {
	type HashType = FakeHash;
	type Transaction = FakeTransaction;
	type Sink = FakeTransactionsSink;
	type PayloadBuilder = ();

	async fn build_transaction<'a>(
		&self,
		watched: bool,
		params: BuildTransactionParams<'a>,
		sink: &Self::Sink,
		_payload_builder: &Self::PayloadBuilder,
	) -> DefaultTxTask<Self::Transaction> {
		if watched {
			todo!()
		};
		let mut nonces = sink.nonces.write();
		let nonce = if let Some(nonce) = nonces.get_mut(&hex::encode(params.account)) {
			*nonce += 1;
			*nonce
		} else {
			nonces.insert(hex::encode(params.account), 0);
			0
		};
		let id = params.account.parse::<u32>().ok();

		if let Some(i) = id {
			DefaultTxTask::<FakeTransaction>::new_watched(FakeTransaction::new_multiple(
				i,
				nonce,
				vec![],
			))
		} else {
			DefaultTxTask::<FakeTransaction>::new_watched(FakeTransaction::new_with_keyring(
				"alice".to_string(),
				nonce,
				vec![],
			))
		}
	}
}

/// What account was used to sign transaction
#[derive(Default, PartialEq, Debug, Clone, Serialize, Deserialize)]
pub enum AccountMetadata {
	/// Holds index used for account derivation
	#[default]
	None,
	Derived(u32),
	KeyRing(String),
}

/// Type of transaction logic.
#[derive(Clone)]
pub enum TransactionCall {
	Transfer,
	Remark(u32),
}

#[derive(Clone)]
/// Type of transaction to execute.
pub struct TransactionRecipe {
	pub(crate) call: TransactionCall,
}

impl TransactionRecipe {
	pub fn transfer() -> Self {
		Self { call: TransactionCall::Transfer }
	}

	pub fn remark(size: u32) -> Self {
		Self { call: TransactionCall::Remark(size) }
	}
}

/// Interface that asks for logic to decide if a transaction is done.
pub(crate) trait TransactionStatusIsDone {
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
	pub(crate) fn get_letter(&self) -> char {
		match self {
			TransactionStatus::Validated => 'V',
			TransactionStatus::Broadcasted => 'b',
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

impl<H: BlockHash> TransactionStatusIsDone for TransactionStatus<H> {
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

/// Interface for a multi-chain transaction abstraction.
pub trait Transaction: Send + Sync {
	type HashType: BlockHash + 'static;
	fn hash(&self) -> Self::HashType;
	fn as_any(&self) -> &dyn Any;
	fn nonce(&self) -> u128;
	fn valid_until(&self) -> &Option<u64>;
	fn account_metadata(&self) -> AccountMetadata;
}

/// Interface for monitoring transaction state.
#[async_trait]
pub trait TransactionMonitor<H: BlockHash> {
	/// Wait for the transaction to finalize.
	///
	/// An optional block number is given to be considered for waiting when needed.
	async fn wait(&self, tx_hash: H, until: Option<u64>) -> Result<H, Error>;
}

/// Abstraction for RPC client
#[async_trait]
pub trait TransactionsSink<H: BlockHash>: Send + Sync {
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = H>,
	) -> Result<StreamOf<TransactionStatus<H>>, Error>;

	async fn submit(&self, tx: &dyn Transaction<HashType = H>) -> Result<H, Error>;

	///Current count of transactions being processed by sink
	async fn pending_extrinsics(&self) -> usize;

	fn transaction_monitor(&self) -> Option<&dyn TransactionMonitor<H>>;
}
