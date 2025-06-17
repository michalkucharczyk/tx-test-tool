// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

//! Provides transactions sending scenarios together with associated builders and executors.

use std::{collections::HashMap, ops::Range, sync::Arc, time::Duration};

use clap::{Subcommand, ValueEnum};
use futures::executor::block_on;
use subxt::{config::BlockHash, utils::H256};
use tokio::sync::mpsc::Sender;

use crate::{
	block_monitor::BlockMonitor,
	execution_log::TransactionExecutionLog,
	runner::{DefaultTxTask, Runner, TxTask},
	subxt_transaction::{
		generate_ecdsa_keypair, generate_sr25519_keypair, EthTransaction, EthTransactionsSink,
		SubstrateTransaction, SubstrateTransactionsSink,
	},
	transaction::{
		EthTransactionBuilder, SubstrateTransactionBuilder, Transaction, TransactionBuilder,
		TransactionRecipe, TransactionsSink,
	},
};

#[derive(Clone, Debug)]
/// Holds information relevant for transaction generation.
pub(crate) struct TransactionBuildParams {
	pub account: String,
	pub nonce: Option<u128>,
	pub mortality: Option<u64>,
}

#[derive(Debug, Clone)]
/// Describes the account types that will participate
/// in a [`ScenarioType`].
pub enum AccountsDescription {
	Keyring(String),
	Derived(Range<u32>),
}

#[derive(ValueEnum, Clone)]
pub enum ChainType {
	/// Substrate compatible chain.
	Sub,
	/// Etheruem compatible chain.
	Eth,
	/// A fake chain used for experiments & tests.
	Fake,
}

#[derive(Subcommand, Clone)]
/// This enum represents different transactions sending scenarios.
pub enum ScenarioType {
	/// Send single transaction to the node.
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

pub type EthScenarioRunner = Runner<DefaultTxTask<EthTransaction>, EthTransactionsSink>;
pub struct EthScenarioExecutor {
	stop_sender: Sender<()>,
	runner: EthScenarioRunner,
}

pub type SubstrateScenarioRunner =
	Runner<DefaultTxTask<SubstrateTransaction>, SubstrateTransactionsSink>;
pub struct SubstrateScenarioExecutor {
	stop_sender: Sender<()>,
	runner: SubstrateScenarioRunner,
}

impl SubstrateScenarioExecutor {
	pub(crate) fn new(stop_sender: Sender<()>, runner: SubstrateScenarioRunner) -> Self {
		SubstrateScenarioExecutor { stop_sender, runner }
	}
}

impl EthScenarioExecutor {
	pub(crate) fn new(stop_sender: Sender<()>, runner: EthScenarioRunner) -> Self {
		EthScenarioExecutor { stop_sender, runner }
	}
}

/// Multi-chain scenario executor.
pub enum ScenarioExecutor {
	Eth(EthScenarioExecutor),
	Substrate(SubstrateScenarioExecutor),
}

impl ScenarioExecutor {
	/// Executes the encapsulated scenario to send out transactions.
	///
	/// Executes the set of transaction sending tasks, and follows a transaction status on the node
	/// side until a final state is reached.
	///
	/// It returns a mapping of transaction hashes to their respective execution log entries,
	/// providing a detailed view of the transaction's execution process.
	///
	/// It is subject to the configured timeout, and if it will be reached, will return a subset of
	/// the execution logs.
	pub async fn execute(self) -> HashMap<H256, Arc<TransactionExecutionLog<H256>>> {
		match self {
			ScenarioExecutor::Eth(mut inner) => inner.runner.run().await,
			ScenarioExecutor::Substrate(mut inner) => inner.runner.run().await,
		}
	}

	/// Installs a ctrl_c handler which sends a stop notification on the executor
	/// stop sender channel, to notify the stop of the scenario for displaying partial stats about
	/// the transactions execution.
	///
	/// Can be called only once for the lifetime of a the program execution.
	fn install_ctrlc_stop_hook(&self) {
		let stop_sender = match &self {
			ScenarioExecutor::Eth(inner) => inner.stop_sender.clone(),
			ScenarioExecutor::Substrate(inner) => inner.stop_sender.clone(),
		};
		ctrlc::set_handler(move || {
			block_on(stop_sender.send(())).expect("Could not send signal on channel.")
		})
		.expect("Error setting Ctrl-C handler");
	}
}

/// Building logic for the execution of a scenario.
pub struct ScenarioBuilder {
	account_id: Option<String>,
	start_id: Option<u32>,
	last_id: Option<u32>,
	nonce_from: Option<u128>,
	txs_count: u32,
	tx_recipe: Option<TransactionRecipe>,
	mortality: Option<u64>,
	does_block_monitoring: bool,
	watched_txs: bool,
	send_threshold: Option<usize>,
	rpc_uri: Option<String>,
	chain_type: Option<ChainType>,
	installs_ctrl_c_stop_hook: bool,
	executor_id: Option<String>,
	tip: u128,
	log_file_name_prefix: Option<String>,
	base_dir_path: Option<String>,
	timeout: Option<Duration>,
	use_legacy_backend: bool,
}

impl Default for ScenarioBuilder {
	fn default() -> Self {
		Self::new()
	}
}

impl ScenarioBuilder {
	/// A default initializer of the builder, with a few defaults:
	/// - `tx_recipe` is set to [`crate::transaction::TransactionCall::Transfer`]
	/// - `does_block_monitoring` is set to `false`.
	/// - `installs_ctrl_c_stop_hook` is set to `false`.
	/// - `send_threshold` is set to `1000`.
	pub fn new() -> Self {
		ScenarioBuilder {
			account_id: None,
			start_id: None,
			last_id: None,
			nonce_from: None,
			txs_count: 1,
			tx_recipe: Some(TransactionRecipe::transfer(0)),
			does_block_monitoring: false,
			mortality: None,
			watched_txs: false,
			send_threshold: Some(1000),
			rpc_uri: None,
			chain_type: None,
			installs_ctrl_c_stop_hook: false,
			executor_id: None,
			tip: 0,
			log_file_name_prefix: None,
			base_dir_path: None,
			timeout: None,
			use_legacy_backend: false,
		}
	}

	/// Configure the account id for building a batch of transactions based on a single signer.
	/// The setter parameter is a string that can be in the form of a number, in which case it will
	/// behave the same as using `with_start_id` (without `with_last_id`), but it can receive
	/// a derivation path like the usual Polkadot development accounts (e.g. "alice", "bob", etc).
	pub fn with_account_id(mut self, account_id: String) -> Self {
		self.account_id = Some(account_id);
		self
	}

	/// Configure the account id for the first signer used for the transactions building.
	/// If the builder isn't configured with a last signer account id, then the scenario
	/// builder will build transactions only for the account specified with this setter.
	///
	/// It is usually used in pair with `with_last_id`, to set an ids range where each id will be
	/// the last part of a derivation path used for multiple accounts generation, each being a
	/// signer for a batch of transactions.
	pub fn with_start_id(mut self, start_id: u32) -> Self {
		self.start_id = Some(start_id);
		self
	}

	/// Last id of an account signer that is also representing the end of an ids range,
	/// each id being the last part of a derivation path used to generate accounts that sign a set
	/// of transactions (see
	/// [`crate::subxt_transaction::derive_accounts`]).
	pub fn with_last_id(mut self, last_id: u32) -> Self {
		self.last_id = Some(last_id);
		self
	}

	/// The start of a nonce counter that's incremented with each built transaction, in relation to
	/// a specific signer account (that can be also part of a range of accounts as it happens when
	/// both `start_id` and `last_id` parameters of the builder are set), while the number of the
	/// built transactions is lower than `txs_count`.
	pub fn with_nonce_from(mut self, nonce_from: Option<u128>) -> Self {
		self.nonce_from = nonce_from;
		self
	}

	/// The number of the transactions that will be built in relation to a signer account.
	pub fn with_txs_count(mut self, txs_count: u32) -> Self {
		self.txs_count = txs_count;
		self
	}

	/// Sets transaction recipe to a regular balances transfer.
	///
	/// The builder is already initialised with a transfer transaction recipe with a tip of 0.
	/// If a tip is set, the builder will update the tip of the transaction recipe accordingly.
	pub fn with_transfer_recipe(mut self) -> Self {
		self.tx_recipe = Some(TransactionRecipe::transfer(self.tip));
		self
	}

	/// Set a remark transaction recipe.
	///
	/// If a tip is set, the builder will update the tip of the transaction recipe
	/// accordingly.
	pub fn with_remark_recipe(mut self, remark: u32) -> Self {
		self.tx_recipe = Some(TransactionRecipe::remark(remark, self.tip));
		self
	}

	/// Allows to specify transaction tip. This indirectly controls priority of transaction.
	pub fn with_tip(mut self, tip: u128) -> Self {
		if let Some(r) = self.tx_recipe.as_mut() {
			r.tip = tip
		};
		self.tip = tip;
		self
	}

	/// Spawns block monitor. Allows to monitor the transaction finalization status for unwatched
	/// transactions.
	pub fn with_block_monitoring(mut self, does_block_monitoring: bool) -> Self {
		self.does_block_monitoring = does_block_monitoring;
		self
	}

	/// Sets for how many blocks a transaction is considered valid, and expected to finalize.
	/// Note: using this setter can increase the transaction creation times which can impact heavy
	/// load tests that create millions of transactions. This method instructs a scenario to use
	/// an online client for txs creation, since creating mortal txs requires knowledge about the
	/// last finalized block on chain.
	pub fn with_mortality(mut self, mortality: u64) -> Self {
		self.mortality = Some(mortality);
		self
	}

	/// Defines the URI of the node where transactions are dispatched.
	pub fn with_rpc_uri(mut self, rpc_uri: String) -> Self {
		self.rpc_uri = Some(rpc_uri);
		self
	}

	/// Send transactions using `submit_and_watch` method. Progress of all transcations will be
	/// monitored. If using unwatched transaction `Self::with_block_monitoring` may be useful for
	/// tracking finalization of transactions.
	pub fn with_watched_txs(mut self, watched_txs: bool) -> Self {
		self.watched_txs = watched_txs;
		self
	}

	/// Allows to specify the chain type.
	pub fn with_chain_type(mut self, chain_type: ChainType) -> Self {
		self.chain_type = Some(chain_type);
		self
	}

	/// Specifies how many transactions in transaction pool on the node side will be maintained at
	/// the fork of the best chain.
	///
	/// `usize::MAX` means that the count of `pending_extrinsics` on node side is not called, and an
	/// executor will send as much as possible.
	pub fn with_send_threshold(mut self, send_threshold: usize) -> Self {
		self.send_threshold = Some(send_threshold);
		self
	}

	/// If specified, the stats will be printed when `stop` signal is sent to process.
	pub fn with_installed_ctrlc_stop_hook(mut self, installs_ctrl_c_stop_hook: bool) -> Self {
		self.installs_ctrl_c_stop_hook = installs_ctrl_c_stop_hook;
		self
	}

	/// Sets a maximum duration for the scenario execution.  
	///
	/// If specified, execution will be limited to the given timeout, ensuring  the executor returns
	/// with logs if the duration is reached.  Typically, the scenario will complete earlier, but
	/// the timeout acts  as a safeguard to prevent indefinite execution.
	pub fn with_timeout_in_secs(mut self, secs: u64) -> Self {
		self.timeout = Some(Duration::from_secs(secs));
		self
	}

	/// Defines the log prefix for the executor instance being built.
	pub fn with_executor_id(mut self, executor_id: String) -> Self {
		self.executor_id = Some(executor_id);
		self
	}

	/// Defines the prefix of the log name.
	pub fn with_log_file_name_prefix(mut self, log_file_name_prefix: String) -> Self {
		self.log_file_name_prefix = Some(log_file_name_prefix);
		self
	}

	/// Defines the path of the directory where the log file will be stored.
	pub fn with_base_dir_path(mut self, base_dir_path: String) -> Self {
		self.base_dir_path = Some(base_dir_path);
		self
	}

	/// Use legacy backend. In some scenarios using this may help overcome some RPC related
	/// problems. Shall be removed in some point in future.
	pub fn with_legacy_backend(mut self, use_legacy_backend: bool) -> Self {
		self.use_legacy_backend = use_legacy_backend;
		self
	}

	/// Returns a set of tasks that handle transaction execution.
	async fn build_transactions<H, T, S, B>(&self, builder: B, sink: S) -> Vec<DefaultTxTask<T>>
	where
		H: BlockHash + 'static,
		T: Transaction<HashType = H> + Send + 'static,
		S: TransactionsSink<H> + 'static + Clone,
		B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
	{
		let mut tx_build_params = vec![];
		if let Some(start_id) = self.start_id {
			let last_id = self.last_id.unwrap_or(start_id);
			for account in start_id..=last_id {
				let mut nonce = self.nonce_from;
				for _ in 0..self.txs_count {
					tx_build_params.push(TransactionBuildParams {
						account: account.to_string(),
						nonce,
						mortality: self.mortality,
					});
					nonce = nonce.map(|n| n + 1);
				}
			}
		} else {
			let mut nonce = self.nonce_from;
			let account = self
				.account_id
				.clone()
				.expect("to have configured an account id for transactions generation");
			for _ in 0..self.txs_count {
				tx_build_params.push(TransactionBuildParams {
					account: account.clone(),
					nonce,
					mortality: self.mortality,
				});
				nonce = nonce.map(|n| n + 1);
			}
		}

		let n = tx_build_params.len();
		let t = std::cmp::min(
			n,
			std::thread::available_parallelism().unwrap_or(1usize.try_into().unwrap()).get(),
		);

		let tx_build_params = Arc::<Vec<TransactionBuildParams>>::from(tx_build_params);
		let builder = Arc::new(builder);
		let mut threads = Vec::new();

		(0..t).for_each(|thread_idx| {
			let chunk = ((thread_idx * n) / t)..(((thread_idx + 1) * n) / t);
			let tx_build_params = tx_build_params.clone();
			let builder = builder.clone();
			let sink = sink.clone();
			let recipe = self
				.tx_recipe
				.clone()
				.expect("to be configured with a transaction recipe. qed.");
			let watched_txs = self.watched_txs;
			threads.push(tokio::task::spawn(async move {
				let mut txs = vec![];
				for i in chunk {
					let build_params = tx_build_params[i].clone();
					txs.push(
						builder
							.build_transaction(
								&build_params.account,
								&build_params.nonce,
								&build_params.mortality,
								&sink,
								watched_txs,
								&recipe,
							)
							.await,
					);
				}
				txs
			}));
		});

		let mut results = vec![];
		for handle in threads {
			let result = handle.await.unwrap();
			results.push(result);
		}
		let mut txs: Vec<_> = results.into_iter().flatten().collect();
		txs.sort_by_key(|k| k.tx().nonce());
		txs
	}

	/// Returns a runner of transactions for the configured scenario.
	pub async fn build(self) -> ScenarioExecutor {
		let does_block_monitoring = self.does_block_monitoring;
		let send_threshold =
			self.send_threshold.expect("to have configured the send threshold. qed.");
		let rpc_uri = self.rpc_uri.clone().expect("to have configured the rpc uri. qed.");
		let chain_type = self.chain_type.clone().expect("to have a configured chain type. qed");
		let accounts_description = if let Some(start_id) = self.start_id {
			let last_id = self.last_id.unwrap_or(start_id);
			AccountsDescription::Derived(start_id..last_id + 1)
		} else if let Some(account_description) = self
			.account_id
			.clone()
			.and_then(|id| id.parse::<u32>().ok())
			.map(|id| AccountsDescription::Derived(id..id + 1))
		{
			account_description
		} else {
			AccountsDescription::Keyring(
				self.account_id
					.clone()
					.expect("to have configured an account id for transactions generation"),
			)
		};

		let installs_ctrlc_stop_hook = self.installs_ctrl_c_stop_hook;
		match chain_type {
			ChainType::Eth => {
				let builder = EthTransactionBuilder::default();
				let new_with_uri_with_accounts_description =
					EthTransactionsSink::new_with_uri_with_accounts_description(
						rpc_uri.as_str(),
						accounts_description,
						generate_ecdsa_keypair,
						if does_block_monitoring {
							Some(BlockMonitor::new(rpc_uri.as_str()).await)
						} else {
							None
						},
						self.use_legacy_backend,
					);
				let sink = new_with_uri_with_accounts_description.await;
				let txs = self.build_transactions(builder, sink.clone()).await;
				let (stop_sender, runner) =
					Runner::<DefaultTxTask<EthTransaction>, EthTransactionsSink>::new(
						send_threshold,
						sink,
						txs.into_iter().rev().collect(),
						self.log_file_name_prefix,
						self.base_dir_path,
						self.executor_id,
						self.timeout,
					);
				let executor = ScenarioExecutor::Eth(EthScenarioExecutor::new(stop_sender, runner));
				installs_ctrlc_stop_hook.then(|| executor.install_ctrlc_stop_hook());
				executor
			},
			ChainType::Sub => {
				let builder = SubstrateTransactionBuilder::default();
				let sink = SubstrateTransactionsSink::new_with_uri_with_accounts_description(
					rpc_uri.as_str(),
					accounts_description,
					generate_sr25519_keypair,
					if does_block_monitoring {
						Some(BlockMonitor::new(rpc_uri.as_str()).await)
					} else {
						None
					},
					self.use_legacy_backend,
				)
				.await;
				let txs = self.build_transactions(builder, sink.clone()).await;
				let (stop_sender, runner) =
					Runner::<DefaultTxTask<SubstrateTransaction>, SubstrateTransactionsSink>::new(
						send_threshold,
						sink,
						txs.into_iter().rev().collect(),
						self.log_file_name_prefix,
						self.base_dir_path,
						self.executor_id,
						self.timeout,
					);

				let executor = ScenarioExecutor::Substrate(SubstrateScenarioExecutor::new(
					stop_sender,
					runner,
				));
				installs_ctrlc_stop_hook.then(|| executor.install_ctrlc_stop_hook());
				executor
			},
			ChainType::Fake => unimplemented!(),
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::{
		fake_transaction_sink::FakeTransactionsSink,
		scenario::ScenarioBuilder,
		transaction::{AccountMetadata, FakeTransactionBuilder},
	};

	use crate::{runner::TxTask, transaction::Transaction};

	#[tokio::test]
	async fn build_tx_tasks_based_on_scenario_type() {
		// One shot from derived account based on number id.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder;
		let scenario_builder = ScenarioBuilder::new().with_start_id(0).with_nonce_from(Some(0));
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 1);
		assert_eq!(tasks[0].tx().nonce(), 0);
		assert_eq!(tasks[0].tx().account_metadata(), AccountMetadata::Derived(0));

		// One shot from derived account.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder;
		let scenario_builder = ScenarioBuilder::new()
			.with_account_id("alice".to_string())
			.with_nonce_from(Some(0));
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 1);
		assert_eq!(tasks[0].tx().nonce(), 0);
		assert_eq!(tasks[0].tx().account_metadata(), AccountMetadata::KeyRing("alice".to_string()));

		// Build from single derived account based on number id.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder;
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id(1)
			.with_nonce_from(Some(0))
			.with_txs_count(10);
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 10);
		for (i, task) in tasks.iter().enumerate() {
			assert_eq!(task.tx().nonce(), i as u128);
			assert_eq!(task.tx().account_metadata(), AccountMetadata::Derived(1));
		}

		// Buld from single account keyring.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder;
		let scenario_builder = ScenarioBuilder::new()
			.with_account_id("alice".to_string())
			.with_nonce_from(Some(0))
			.with_txs_count(10);
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 10);
		for (i, task) in tasks.iter().enumerate() {
			assert_eq!(task.tx().nonce(), i as u128);
			assert_eq!(task.tx().account_metadata(), AccountMetadata::KeyRing("alice".to_string()));
		}

		// Buld from many derived accounts based on number ids.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder;
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id(5)
			.with_last_id(10)
			.with_nonce_from(Some(0))
			.with_txs_count(10);
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 60);
		for (i, task) in tasks.iter().enumerate() {
			assert_eq!(task.tx().nonce(), i as u128 / 6);
			assert_eq!(task.tx().account_metadata(), AccountMetadata::Derived((i as u32 % 6) + 5));
		}
	}
}
