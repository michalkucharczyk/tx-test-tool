//! This is an experimental library that can be used to send transactions
//! to a substrate network and monitor them in terms of how many have been
//! submitted to the transaction pool, validated, broadcasted, inlcluded in blocks,
//! finalized, invalid or dropped.
//!
//! There is an associated binary that can be used as a CLI, an alternative to using
//! the library.
//!
//! Example:
//!  ```rust,ignore
//!  fn test() {
//!	    // Shared params.
//!     let send_threshold = 20_000;
//!     let ws = "ws://127.0.0.1:9933";
//!     let recipe_future = TransactionRecipe::transfer();
//!     let recipe_ready = recipe_future.clone();
//!     let block_monitor = false;
//!     let unwatched = false;
//!
//!     // Scenarios and sinks.
//!     let scenario_future =
//!       SendingScenario::FromManyAccounts { start_id: 0, last_id: 99, from: Some(100), count: 100
//! };     let scenario_ready =
//!       SendingScenario::FromManyAccounts { start_id: 0, last_id: 99, from: Some(0), count: 100 };
//
//!     let future_scenario_planner =
//!	      ScenarioPlanner::new(ws, scenario_future, recipe_future, block_monitor).await;
//!     let ready_scenario_planner =
//!	      ScenarioPlanner::new(ws, scenario_ready, recipe_ready, block_monitor).await;
//!
//!     let ((future_stop_runner_tx, mut runner_future), future_queue_task) =
//!	      RunnerFactory::substrate_runner(future_scenario_planner, send_threshold,
//! unwatched).await; 	    let ((ready_stop_runner_tx, mut runner_ready), ready_queue_task) =
//!	      RunnerFactory::substrate_runner(ready_scenario_planner, send_threshold, unwatched).await;
//!
//!     let (future_logs, ready_logs) = join(runner_future.run_poc2(),
//! runner_ready.run()).await;     let finalized_future =
//!	      future_logs.values().filter_map(|default_log| default_log.finalized()).count();
//!     let finalized_ready =
//!	      ready_logs.values().filter_map(|default_log| default_log.finalized()).count();
//!     assert_eq!(finalized_future, 100);
//!     assert_eq!(finalized_ready, 100);
//!  }
//!  ```

use crate::transaction::Transaction;

use crate::{
	fake_transaction::{FakeHash, FakeTransaction},
	fake_transaction_sink::FakeTransactionsSink,
	runner::DefaultTxTask,
	subxt_transaction::{
		build_eth_tx_payload, build_substrate_tx_payload, build_subxt_tx, EthRuntimeConfig,
		EthTransaction, EthTransactionsSink, HashOf, SubstrateTransaction,
		SubstrateTransactionsSink,
	},
	transaction::{TransactionRecipe, TransactionsSink},
};
use async_trait::async_trait;
use subxt::{config::BlockHash, PolkadotConfig};

pub mod block_monitor;
pub mod cli;
pub mod error;
pub mod execution_log;
pub mod fake_transaction;
pub mod fake_transaction_sink;
pub mod resubmission;
pub mod runner;
pub mod scenario;
pub mod subxt_api_connector;
pub mod subxt_transaction;
pub mod transaction;

pub fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		use tracing::Metadata;
		use tracing_subscriber::{
			fmt,
			layer::{Context, Filter, SubscriberExt},
			registry, EnvFilter, Layer,
		};

		let filter = EnvFilter::from_default_env();

		struct F {
			env_filter: EnvFilter,
		}

		impl Default for F {
			fn default() -> Self {
				Self { env_filter: EnvFilter::from_default_env() }
			}
		}
		impl<S> Filter<S> for F {
			fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
				!self.env_filter.enabled(meta, cx.clone()) &&
					meta.target() == execution_log::STAT_TARGET
			}
		}

		let debug_layer = fmt::layer().with_target(true).with_filter(filter);
		let stat_layer = fmt::layer()
			.with_target(false)
			.with_level(false)
			.without_time()
			.with_filter(F::default());

		let subscriber = registry::Registry::default().with(debug_layer).with(stat_layer);

		tracing::subscriber::set_global_default(subscriber)
			.expect("Unable to set a global subscriber");
	});
}

#[async_trait]
pub trait TransactionBuilder {
	type HashType: BlockHash;
	type Transaction: Transaction<HashType = Self::HashType>;
	type Sink: TransactionsSink<Self::HashType>;

	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		recipe: &TransactionRecipe,
	) -> DefaultTxTask<Self::Transaction>;
}

#[derive(Default)]
pub struct FakeTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for FakeTransactionBuilder {
	type HashType = FakeHash;
	type Transaction = FakeTransaction;
	type Sink = FakeTransactionsSink;
	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		_nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		_recipe: &TransactionRecipe,
	) -> DefaultTxTask<Self::Transaction> {
		if unwatched {
			todo!()
		};
		let mut nonces = sink.nonces.write();
		let nonce = if let Some(nonce) = nonces.get_mut(&hex::encode(account)) {
			*nonce += 1;
			*nonce
		} else {
			nonces.insert(hex::encode(account), 0);
			0
		};
		let i = account.parse::<u32>().expect("Account shall be valid integer");
		// DefaultTxTask::<FakeTransaction>::new_watched(FakeTransaction::new_droppable_loop(
		// 	i + (nonce << 16) as u32,
		// 	200,
		// ))
		DefaultTxTask::<FakeTransaction>::new_watched(FakeTransaction::new_droppable_2nd_success(
			i + (nonce << 16) as u32,
			1000,
		))
		// FakeTransaction::new_droppable_2nd_success(i, 0)
	}
}

#[derive(Default)]
pub struct SubstrateTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for SubstrateTransactionBuilder {
	type HashType = HashOf<PolkadotConfig>;
	type Transaction = SubstrateTransaction;
	type Sink = SubstrateTransactionsSink;
	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		recipe: &TransactionRecipe,
	) -> DefaultTxTask<Self::Transaction> {
		if unwatched {
			DefaultTxTask::<Self::Transaction>::new_unwatched(
				build_subxt_tx(account, nonce, sink, recipe, build_substrate_tx_payload).await,
			)
		} else {
			DefaultTxTask::<Self::Transaction>::new_watched(
				build_subxt_tx(account, nonce, sink, recipe, build_substrate_tx_payload).await,
			)
		}
	}
}

#[derive(Default)]
pub struct EthTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for EthTransactionBuilder {
	type HashType = HashOf<EthRuntimeConfig>;
	type Transaction = EthTransaction;
	type Sink = EthTransactionsSink;
	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		recipe: &TransactionRecipe,
	) -> DefaultTxTask<Self::Transaction> {
		if unwatched {
			DefaultTxTask::<Self::Transaction>::new_unwatched(
				build_subxt_tx(account, nonce, sink, recipe, build_eth_tx_payload).await,
			)
		} else {
			DefaultTxTask::<Self::Transaction>::new_watched(
				build_subxt_tx(account, nonce, sink, recipe, build_eth_tx_payload).await,
			)
		}
	}
}
