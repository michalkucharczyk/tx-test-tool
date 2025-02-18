use std::{collections::HashMap, ops::Range, sync::Arc};

use clap::{Subcommand, ValueEnum};
use futures::executor::block_on;
use subxt::{config::BlockHash, utils::H256};
use tokio::sync::mpsc::Sender;

use crate::{
	block_monitor::BlockMonitor,
	execution_log::DefaultExecutionLog,
	resubmission::{DefaultResubmissionQueue, ResubmissionQueueTask},
	runner::{DefaultTxTask, Runner, TxTask},
	subxt_transaction::{
		generate_ecdsa_keypair, generate_sr25519_keypair, EthTransaction, EthTransactionsSink,
		SubstrateTransaction, SubstrateTransactionsSink,
	},
	transaction::{
		EthTransactionBuilder, ResubmitHandler, SubstrateTransactionBuilder, Transaction,
		TransactionBuilder, TransactionRecipe, TransactionsSink,
	},
};

#[derive(Clone, Debug)]
/// Holds information relevant for transaction generation.
pub(crate) struct TransactionBuildParams {
	pub account: String,
	pub nonce: Option<u128>,
}

#[derive(Debug, Clone)]
/// Describes the account types that will participate
/// in a [`SendingScenario`].
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
/// Send transactions to the node using different scenarios.
pub enum ScenarioType {
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

pub type EthScenarioRunner = Runner<
	DefaultTxTask<EthTransaction>,
	EthTransactionsSink,
	DefaultResubmissionQueue<DefaultTxTask<EthTransaction>>,
>;
pub struct EthScenarioExecutor {
	stop_sender: Sender<()>,
	runner: EthScenarioRunner,
	resubmission_queue: ResubmissionQueueTask,
}

pub type SubstrateScenarioRunner = Runner<
	DefaultTxTask<SubstrateTransaction>,
	SubstrateTransactionsSink,
	DefaultResubmissionQueue<DefaultTxTask<SubstrateTransaction>>,
>;
pub struct SubstrateScenarioExecutor {
	stop_sender: Sender<()>,
	runner: SubstrateScenarioRunner,
	resubmission_queue: ResubmissionQueueTask,
}

impl SubstrateScenarioExecutor {
	pub(crate) fn new(
		stop_sender: Sender<()>,
		runner: SubstrateScenarioRunner,
		resubmission_queue: ResubmissionQueueTask,
	) -> Self {
		SubstrateScenarioExecutor { stop_sender, runner, resubmission_queue }
	}
}

impl EthScenarioExecutor {
	pub fn new(
		stop_sender: Sender<()>,
		runner: EthScenarioRunner,
		resubmission_queue: ResubmissionQueueTask,
	) -> Self {
		EthScenarioExecutor { stop_sender, runner, resubmission_queue }
	}
}

/// Multi-chain scenario executor.
pub enum ScenarioExecutor {
	Eth(EthScenarioExecutor),
	Substrate(SubstrateScenarioExecutor),
}

impl ScenarioExecutor {
	/// Executes the set of tasks following a transaction on chain, until a final state
	/// ['TransactionStatusIsDone`].
	pub async fn execute<T: TxTask + 'static>(
		self,
	) -> HashMap<H256, Arc<DefaultExecutionLog<H256>>> {
		match self {
			ScenarioExecutor::Eth(mut inner) => {
				let (_, logs) =
					futures::future::join(inner.resubmission_queue, inner.runner.run()).await;
				logs
			},
			ScenarioExecutor::Substrate(mut inner) => {
				let (_, logs) =
					futures::future::join(inner.resubmission_queue, inner.runner.run()).await;
				logs
			},
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
	start_id: Option<String>,
	last_id: Option<u32>,
	nonce_from: Option<u128>,
	txs_count: u32,
	tx_recipe: Option<TransactionRecipe>,
	does_block_monitoring: bool,
	watched_txs: bool,
	send_threshold: Option<usize>,
	rpc_uri: Option<String>,
	chain_type: Option<ChainType>,
	installs_ctrl_c_stop_hook: bool,
}

impl ScenarioBuilder {
	/// A default initializer of the builder, with a few defaults:
	/// - `tx_recipe` is set to [`TransactionCall::Transfer`]
	/// - `does_block_monitoring` is set to `false`.
	/// - `installs_ctrl_c_stop_hook` is set to `false`.
	/// - `send_threshold` is set to `1000`.
	pub fn new() -> Self {
		ScenarioBuilder {
			start_id: None,
			last_id: None,
			nonce_from: None,
			txs_count: 1,
			tx_recipe: Some(TransactionRecipe::transfer()),
			does_block_monitoring: false,
			watched_txs: false,
			send_threshold: Some(1000),
			rpc_uri: None,
			chain_type: None,
			installs_ctrl_c_stop_hook: false,
		}
	}

	/// Configure the account that represents the first signer used for the transactions
	/// building. If the account is representing a number, and the builder isn't configured with a
	/// last account to sign the last batch of transactions planned for execution, then the scenario
	/// builder will build transactions only for the account specified with this setter.
	///
	/// The parameter is a string that can represent a number or a default development account
	/// derivation path (e.g. "alice", "bob" etc.). If a number is given (e.g. "0", "1", "2"), it
	/// will be interpreted as the last part of a default derivation path (see
	/// [`subxt_transaction::derive_accounts`]), used to generate signer accounts, which are assumed
	/// to be initialised as development accounts with balances by the networks where the
	/// transactions will be executed.
	pub fn with_start_id(mut self, start_id: String) -> Self {
		self.start_id = Some(start_id);
		self
	}

	/// Last id of an account signer that is also representing the end of an ids range,
	/// each id being the last part of a derivation path used to generate accounts that sign a set
	/// of transactions (see
	/// [`subxt_transaction::derive_accounts`]). It is optional and if not set only the `start_id`
	/// will be considered as a single signer account used to build/execute transactions.
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

	/// The builder is already initialised with a transfer transaction recipe for the built
	/// transactions, and this API
	pub fn with_transfer_tx_recipe(mut self) -> Self {
		self.tx_recipe = Some(TransactionRecipe::transfer());
		self
	}

	pub fn with_remark_tx_recipe(mut self, remark: u32) -> Self {
		self.tx_recipe = Some(TransactionRecipe::remark(remark));
		self
	}

	pub fn with_block_monitoring(mut self, does_block_monitoring: bool) -> Self {
		self.does_block_monitoring = does_block_monitoring;
		self
	}

	pub fn with_rpc_uri(mut self, rpc_uri: String) -> Self {
		self.rpc_uri = Some(rpc_uri);
		self
	}

	pub fn with_watched_txs(mut self, watched_txs: bool) -> Self {
		self.watched_txs = watched_txs;
		self
	}

	pub fn with_chain_type(mut self, chain_type: ChainType) -> Self {
		self.chain_type = Some(chain_type);
		self
	}

	pub fn with_send_threshold(mut self, send_threshold: usize) -> Self {
		self.send_threshold = Some(send_threshold);
		self
	}

	pub fn with_installed_ctrlc_stop_hook(mut self, installs_ctrl_c_stop_hook: bool) -> Self {
		self.installs_ctrl_c_stop_hook = installs_ctrl_c_stop_hook;
		self
	}

	/// Returns a set of tasks that handle transaction execution.
	async fn build_transactions<H, T, S, B>(&self, builder: B, sink: S) -> Vec<DefaultTxTask<T>>
	where
		H: BlockHash + 'static,
		T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
		S: TransactionsSink<H> + 'static + Clone,
		B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
	{
		let mut tx_build_params = vec![];
		if let Some(start_id) = self.start_id.clone().and_then(|inner| inner.parse::<u32>().ok()) {
			let last_id = self.last_id.unwrap_or(start_id);
			for account in start_id..=last_id {
				let mut nonce = self.nonce_from.clone();
				for _ in 0..self.txs_count {
					tx_build_params
						.push(TransactionBuildParams { account: account.to_string(), nonce });
					nonce = nonce.map(|n| n + 1);
				}
			}
		} else {
			let mut nonce = self.nonce_from.clone();
			let account = self.start_id.clone().expect("to have a configured starting account id");
			for _ in 0..self.txs_count {
				tx_build_params.push(TransactionBuildParams { account: account.clone(), nonce });
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
		let accounts_description = if let Some(last_id) = self.last_id {
			self.start_id
				.clone()
				.expect("to have a configured start account id")
				.parse::<u32>()
				.ok()
				.map(|start_id| AccountsDescription::Derived(start_id..last_id))
				.unwrap_or(AccountsDescription::Keyring(
					self.start_id.clone().expect("to have a configured start account id"),
				))
		} else {
			self.start_id
				.clone()
				.expect("to have a configured start account id")
				.parse::<u32>()
				.ok()
				.map(|start_id| AccountsDescription::Derived(start_id..start_id + 1))
				.unwrap_or(AccountsDescription::Keyring(
					self.start_id.clone().expect("to have a configured start account id"),
				))
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
					);
				let sink = new_with_uri_with_accounts_description.await;
				let txs = self.build_transactions(builder, sink.clone()).await;
				let (queue, queue_task) = DefaultResubmissionQueue::new();
				let (stop_sender, runner) =
					Runner::<
						DefaultTxTask<EthTransaction>,
						EthTransactionsSink,
						DefaultResubmissionQueue<DefaultTxTask<EthTransaction>>,
					>::new(send_threshold as usize, sink, txs.into_iter().rev().collect(), queue);
				let executor = ScenarioExecutor::Eth(EthScenarioExecutor::new(
					stop_sender,
					runner,
					queue_task,
				));
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
				)
				.await;
				let txs = self.build_transactions(builder, sink.clone()).await;
				let (queue, queue_task) = DefaultResubmissionQueue::new();
				let (stop_sender, runner) =
					Runner::<
						DefaultTxTask<SubstrateTransaction>,
						SubstrateTransactionsSink,
						DefaultResubmissionQueue<DefaultTxTask<SubstrateTransaction>>,
					>::new(send_threshold, sink, txs.into_iter().rev().collect(), queue);

				let executor = ScenarioExecutor::Substrate(SubstrateScenarioExecutor::new(
					stop_sender,
					runner,
					queue_task,
				));
				installs_ctrlc_stop_hook.then(|| executor.install_ctrlc_stop_hook());
				executor
			},
			ChainType::Fake => todo!(),
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
		let builder = FakeTransactionBuilder::default();
		let scenario_builder =
			ScenarioBuilder::new().with_start_id("0".to_string()).with_nonce_from(Some(0));
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 1);
		assert_eq!(tasks[0].tx().nonce(), 0);
		assert_eq!(tasks[0].tx().account_metadata(), AccountMetadata::Derived(0));

		// One shot from derived account.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder::default();
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id("alice".to_string())
			.with_nonce_from(Some(0));
		let tasks = scenario_builder.build_transactions(builder, sink).await;
		assert_eq!(tasks.len(), 1);
		assert_eq!(tasks[0].tx().nonce(), 0);
		assert_eq!(tasks[0].tx().account_metadata(), AccountMetadata::KeyRing("alice".to_string()));

		// Build from single derived account based on number id.
		let sink = FakeTransactionsSink::default();
		let builder = FakeTransactionBuilder::default();
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id("1".to_string())
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
		let builder = FakeTransactionBuilder::default();
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id("alice".to_string())
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
		let builder = FakeTransactionBuilder::default();
		let scenario_builder = ScenarioBuilder::new()
			.with_start_id("5".to_string())
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
