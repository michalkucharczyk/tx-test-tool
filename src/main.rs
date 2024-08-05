#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

use async_trait::async_trait;
use cli::{ChainType, SendingScenario};
use eth_transaction::{build_eth_tx, EthTransaction, EthTransactionsSink, HashOf};
use fake_transaction::{FakeHash, FakeTransaction};
use fake_transaction_sink::FakeTransactionSink;
use futures::future::join;
use jsonrpsee::core::client::TransportSenderT;
use resubmission::DefaultResubmissionQueue;
use runner::{DefaultTxTask, FakeTxTask, Runner, TxTask};
use std::{any::Any, marker::PhantomData};
use subxt::{config::BlockHash, tx::Signer};
use subxt_transaction::{SubxtTransactionsSink, TransactionSubxt};
use tracing::{debug, info, trace};

mod cli;
mod error;
mod eth_transaction;
mod execution_log;
mod fake_transaction;
mod fake_transaction_sink;
mod resubmission;
mod runner;
mod subxt_api_connector;
mod subxt_transaction;
mod transaction;
use clap::Parser;
use transaction::{ResubmitHandler, Transaction, TransactionsSink};

use crate::{cli::CliCommand, subxt_transaction::EthRuntimeConfig};

fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		use tracing_subscriber::{fmt, layer::SubscriberExt, registry, EnvFilter, Layer};

		let filter = EnvFilter::from_default_env();
		let debug_layer = fmt::layer().with_target(true).with_filter(filter);

		let stat_layer =
			fmt::layer().with_target(false).with_level(false).without_time().with_filter(
				tracing_subscriber::filter::filter_fn(|meta| {
					meta.target() == execution_log::STAT_TARGET
				}),
			);

		let subscriber = registry::Registry::default().with(debug_layer).with(stat_layer);

		tracing::subscriber::set_global_default(subscriber)
			.expect("Unable to set a global subscriber");
	});
}

#[async_trait]
trait TransactionBuilder {
	type HashType: BlockHash;
	type Transaction: Transaction<HashType = Self::HashType>;
	type Sink: TransactionsSink<Self::HashType>;

	async fn build_transaction(
		&self,
		account: &String,
		nonce: &Option<u128>,
		sink: &Self::Sink,
	) -> DefaultTxTask<Self::Transaction>;
}

struct FakeTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for FakeTransactionBuilder {
	type HashType = FakeHash;
	type Transaction = FakeTransaction;
	type Sink = FakeTransactionSink;
	async fn build_transaction(
		&self,
		account: &String,
		nonce: &Option<u128>,
		_: &FakeTransactionSink,
	) -> DefaultTxTask<FakeTransaction> {
		let i = account.parse::<u32>().unwrap();
		DefaultTxTask::<FakeTransaction>::new_watched(FakeTransaction::new_droppable_2nd_success(
			i, 0,
		))
		// FakeTransaction::new_droppable_2nd_success(i, 0)
	}
}

struct EthTransactionBuilder {}

impl EthTransactionBuilder {
	fn new() -> Self {
		Self {}
	}
}

#[async_trait]
impl TransactionBuilder for EthTransactionBuilder {
	type HashType = HashOf<EthRuntimeConfig>;
	type Transaction = EthTransaction;
	type Sink = EthTransactionsSink;
	async fn build_transaction(
		&self,
		account: &String,
		nonce: &Option<u128>,
		sink: &EthTransactionsSink,
	) -> DefaultTxTask<EthTransaction> {
		DefaultTxTask::<EthTransaction>::new_watched(build_eth_tx(account, nonce, sink).await)
		// FakeTransaction::new_droppable_2nd_success(i, 0)
	}
}

impl FakeTransactionBuilder {
	fn new() -> Self {
		Self {}
	}
}

//
// fn generate_transactions(
// 	scenario: TxCommands,
// 	chain_type: ChainType,
// ) -> Vec<Box<dyn GeneratedTransaction>> {
// 	let rpc = FakeTransactionSink::new();
// 	let transactions = (0..100000)
// 		.map(|i| FakeTxTask::new_watched(FakeTransaction::new_droppable_2nd_success(i, 0)))
// 		// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
// 		.collect::<Vec<_>>();
// 	vec![]
// }

async fn execute_scenario<
	H: BlockHash + 'static,
	T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
	S: TransactionsSink<H> + 'static,
	B: TransactionBuilder<HashType = H, Transaction = T, Sink = S>,
>(
	sink: S,
	builder: B,
	scenario: &SendingScenario,
) {
	let transactions = match scenario {
		SendingScenario::OneShot { account, nonce } =>
			vec![builder.build_transaction(account, nonce, &sink).await],
		_ => {
			todo!()
		},
	};

	// let transactions = tx_command.generate_transactions(cli);
	let (queue, queue_task) = DefaultResubmissionQueue::new();
	let mut r = Runner::<DefaultTxTask<T>, S, DefaultResubmissionQueue<DefaultTxTask<T>>>::new(
		100000,
		sink,
		transactions,
		queue,
	);
	join(queue_task, r.run_poc2()).await;
}

fn send_loop() {}

#[tokio::main]
async fn main() {
	init_logger();
	// info!("Hello, world!");
	// debug!(target: "XXX", "Hello, world!");
	// trace!("Hello, world!");

	info!(target: "x", "test");
	debug!(target: "y", "test");
	trace!(target: "z", "test");
	trace!(target: "a", "test");

	let cli = cli::Cli::parse();

	match &cli.command {
		CliCommand::Tx { chain, ws, unwatched, block_monitor, mortal, log_file, scenario } => {
			match chain {
				ChainType::Fake => {
					let sink = FakeTransactionSink::new();
					let builder = FakeTransactionBuilder::new();
					execute_scenario(sink, builder, scenario).await;
				},
				ChainType::Eth => {
					let sink = EthTransactionsSink::new().await;
					let builder = EthTransactionBuilder::new();
					execute_scenario(sink, builder, scenario).await;
				},
				_ => {
					todo!()
				},
			};
		},
		CliCommand::CheckNonce { ws, account } => {
			// Handle check-nonce command
		},
		CliCommand::Metadata { ws } => {
			// Handle metadata command
		},
		CliCommand::BlockMonitor { ws } => {
			// Handle block-monitor command
		},
		CliCommand::LoadLog { log_file, show_graphs, show_errors } => {
			// Handle load-log command
		},
	}
}
