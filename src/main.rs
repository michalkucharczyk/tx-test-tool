#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

use async_trait::async_trait;
use cli::{ChainType, SendingScenario};
use execution_log::{journal::Journal, make_stats};
// use eth_transaction::{build_eth_tx, EthTransaction, EthTransactionsSink};
use fake_transaction::{FakeHash, FakeTransaction};
use fake_transaction_sink::FakeTransactionSink;
use futures::{executor::block_on, future::join};
use jsonrpsee::core::client::TransportSenderT;
use resubmission::DefaultResubmissionQueue;
use runner::{DefaultTxTask, FakeTxTask, Runner, TxTask};
use std::{any::Any, marker::PhantomData};
use subxt::{config::BlockHash, tx::Signer, PolkadotConfig};
use subxt_transaction::{
	build_eth_tx_payload, build_substrate_tx_payload, build_subxt_tx, EthTransaction,
	EthTransactionsSink, HashOf, SubstrateTransaction, SubxtTransaction, SubxtTransactionsSink,
};
use tracing::{debug, info, trace};

mod cli;
mod error;
// mod eth_transaction;
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

use crate::{
	cli::{AccountsDescription, CliCommand},
	execution_log::STAT_TARGET,
	subxt_transaction::{
		generate_ecdsa_keypair, generate_sr25519_keypair, EthRuntimeConfig,
		SubstrateTransactionsSink,
	},
};

fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		use tracing::{debug, info, trace, Metadata};
		use tracing_subscriber::{
			fmt,
			layer::{Context, Filter, SubscriberExt},
			registry, EnvFilter, Layer,
		};

		let filter = EnvFilter::from_default_env();

		struct F {
			env_filter: EnvFilter,
		}
		impl F {
			fn new() -> Self {
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
			.with_filter(F::new());

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
		sink: &Self::Sink,
	) -> DefaultTxTask<Self::Transaction> {
		let mut nonces = sink.nonces.write();
		let nonce = if let Some(nonce) = nonces.get_mut(&hex::encode(account.clone())) {
			*nonce = *nonce + 1;
			*nonce
		} else {
			nonces.insert(hex::encode(account.clone()), 0);
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

struct SubstrateTransactionBuilder {}

impl SubstrateTransactionBuilder {
	fn new() -> Self {
		Self {}
	}
}

#[async_trait]
impl TransactionBuilder for SubstrateTransactionBuilder {
	type HashType = HashOf<PolkadotConfig>;
	type Transaction = SubstrateTransaction;
	type Sink = SubstrateTransactionsSink;
	async fn build_transaction(
		&self,
		account: &String,
		nonce: &Option<u128>,
		sink: &Self::Sink,
	) -> DefaultTxTask<Self::Transaction> {
		DefaultTxTask::<Self::Transaction>::new_watched(
			build_subxt_tx(account, nonce, sink, build_substrate_tx_payload).await,
		)
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
		sink: &Self::Sink,
	) -> DefaultTxTask<Self::Transaction> {
		DefaultTxTask::<Self::Transaction>::new_watched(
			build_subxt_tx(account, nonce, sink, build_eth_tx_payload).await,
		)
	}
}

impl FakeTransactionBuilder {
	fn new() -> Self {
		Self {}
	}
}

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
		SendingScenario::FromSingleAccount { account, from, count } => {
			let mut transactions = vec![];
			let mut nonce = *from;

			for i in 0..*count {
				transactions.push(builder.build_transaction(account, &nonce, &sink).await);
				nonce = nonce.map(|n| n + 1);
			}
			transactions
		},
		SendingScenario::FromManyAccounts { start_id, last_id, from, count } => {
			let mut transactions = vec![];

			for account in *start_id..*last_id {
				let mut nonce = *from;
				for i in 0..*count {
					transactions
						.push(builder.build_transaction(&account.to_string(), &nonce, &sink).await);
					nonce = nonce.map(|n| n + 1);
				}
			}
			transactions
		},
	};

	let transactions = transactions.into_iter().rev().collect();
	// let transactions = tx_command.generate_transactions(cli);
	let (queue, queue_task) = DefaultResubmissionQueue::new();
	let (stop_runner_tx, mut runner) = Runner::<
		DefaultTxTask<T>,
		S,
		DefaultResubmissionQueue<DefaultTxTask<T>>,
	>::new(10000, sink, transactions, queue);

	ctrlc::set_handler(move || {
		block_on(stop_runner_tx.send(())).expect("Could not send signal on channel.")
	})
	.expect("Error setting Ctrl-C handler");
	join(queue_task, runner.run_poc2()).await;
}

fn send_loop() {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	init_logger();

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
					let def = scenario.get_accounts_description();
					let sink = EthTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						def,
						generate_ecdsa_keypair,
					)
					.await;
					let builder = EthTransactionBuilder::new();
					execute_scenario(sink, builder, scenario).await;
				},
				ChainType::Sub => {
					let def = scenario.get_accounts_description();
					let sink = SubstrateTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						def,
						generate_sr25519_keypair,
					)
					.await;
					let builder = SubstrateTransactionBuilder::new();
					execute_scenario(sink, builder, scenario).await;
				},
			};
		},
		CliCommand::CheckNonce { chain, ws, account } => {
			match chain {
				ChainType::Fake => {
					panic!("check nonce not supported for fake chain");
				},
				ChainType::Eth => {
					let desc = if let Ok(id) = account.parse::<u32>() {
						AccountsDescription::Derived(id..id + 1)
					} else {
						AccountsDescription::Keyring(account.clone())
					};
					let sink = EthTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						desc,
						generate_ecdsa_keypair,
					)
					.await;
					let account =
						sink.get_from_account_id(account).ok_or("account shall be correct")?;
					let nonce = sink.check_account_nonce(account).await?;
					info!(target:STAT_TARGET, "{nonce:?}");
				},
				ChainType::Sub => {
					let desc = if let Ok(id) = account.parse::<u32>() {
						AccountsDescription::Derived(id..id + 1)
					} else {
						AccountsDescription::Keyring(account.clone())
					};
					let sink = SubstrateTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						desc,
						generate_sr25519_keypair,
					)
					.await;
					let account =
						sink.get_from_account_id(account).ok_or("account shall be correct")?;
					let nonce = sink.check_account_nonce(account).await?;
					info!(target:STAT_TARGET, "{nonce:?}");
				},
			};
		},
		CliCommand::Metadata { ws } => {
			// Handle metadata command
		},
		CliCommand::BlockMonitor { ws } => {
			// Handle block-monitor command
		},
		CliCommand::LoadLog { chain, log_file, show_graphs, .. } => match chain {
			ChainType::Sub => {
				let logs = Journal::<DefaultTxTask<SubstrateTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
			ChainType::Eth => {
				let logs = Journal::<DefaultTxTask<EthTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
			ChainType::Fake => {
				let logs = Journal::<DefaultTxTask<FakeTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
		},
	};
	Ok(())
}
