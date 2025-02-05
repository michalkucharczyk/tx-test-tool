#![allow(dead_code)]
#![allow(unused_variables)]

use std::{fs, io::BufReader, sync::Arc, time::Duration};

use async_trait::async_trait;
use block_monitor::BlockMonitor;
use cli::{ChainType, SendingScenario};
use execution_log::{journal::Journal, make_stats};
use fake_transaction::{FakeHash, FakeTransaction};
use fake_transaction_sink::FakeTransactionSink;
use futures::{executor::block_on, future::join};
use parity_scale_codec::Compact;
use resubmission::DefaultResubmissionQueue;
use runner::{DefaultTxTask, Runner};
use std::fs::File;
use subxt::{config::BlockHash, ext::frame_metadata::RuntimeMetadataPrefixed, PolkadotConfig};
use subxt_transaction::{
	build_eth_tx_payload, build_substrate_tx_payload, build_subxt_tx, EthTransaction,
	EthTransactionsSink, HashOf, SubstrateTransaction,
};
use tracing::info;

mod block_monitor;
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
use transaction::{ResubmitHandler, Transaction, TransactionRecipe, TransactionsSink};

use crate::{
	cli::{AccountsDescription, CliCommand},
	execution_log::STAT_TARGET,
	runner::TxTask,
	subxt_transaction::{
		generate_ecdsa_keypair, generate_sr25519_keypair, EthRuntimeConfig,
		SubstrateTransactionsSink,
	},
};

fn init_logger() {
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

	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		recipe: &TransactionRecipe,
	) -> DefaultTxTask<Self::Transaction>;
}

struct FakeTransactionBuilder {}

#[async_trait]
impl TransactionBuilder for FakeTransactionBuilder {
	type HashType = FakeHash;
	type Transaction = FakeTransaction;
	type Sink = FakeTransactionSink;
	async fn build_transaction<'a>(
		&self,
		account: &'a str,
		nonce: &Option<u128>,
		sink: &Self::Sink,
		unwatched: bool,
		recipe: &TransactionRecipe,
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

impl FakeTransactionBuilder {
	fn new() -> Self {
		Self {}
	}
}

#[derive(Clone, Debug)]
struct TransactionBuildParams {
	account: String,
	nonce: Option<u128>,
}

async fn build_transactions<H, T, S, B>(
	builder: B,
	sink: S,
	unwatched: bool,
	defs: Vec<TransactionBuildParams>,
	recipe: &TransactionRecipe,
) -> Vec<DefaultTxTask<T>>
where
	H: BlockHash + 'static,
	T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
	S: TransactionsSink<H> + 'static + Clone,
	B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
{
	let n = defs.len();
	let t = std::cmp::min(
		n,
		std::thread::available_parallelism().unwrap_or(1usize.try_into().unwrap()).get(),
	);
	let defs = Arc::<Vec<TransactionBuildParams>>::from(defs);
	let builder = Arc::new(builder);
	let mut threads = Vec::new();

	(0..t).for_each(|thread_idx| {
		let chunk = ((thread_idx * n) / t)..(((thread_idx + 1) * n) / t);
		let defs = defs.clone();
		let builder = builder.clone();
		let sink = sink.clone();
		let recipe = recipe.clone();
		threads.push(tokio::task::spawn(async move {
			let mut txs = vec![];
			for i in chunk {
				let d = defs[i].clone();
				txs.push(
					builder
						.build_transaction(&d.account, &d.nonce, &sink, unwatched, &recipe)
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
	let mut v: Vec<_> = results.into_iter().flatten().collect();
	v.sort_by_key(|k| k.tx().nonce());
	v
}

async fn execute_scenario<H, T, S, B>(
	sink: S,
	builder: B,
	scenario: &SendingScenario,
	unwatched: bool,
	send_threshold: usize,
	recipe: &TransactionRecipe,
) where
	H: BlockHash + 'static,
	T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
	S: TransactionsSink<H> + 'static + Clone,
	B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
{
	let transactions = match scenario {
		SendingScenario::OneShot { account, nonce } =>
			vec![builder.build_transaction(account, nonce, &sink, unwatched, recipe).await],
		SendingScenario::FromSingleAccount { account, from, count } => {
			let mut transactions_defs = vec![];
			let mut nonce = *from;

			for _ in 0..*count {
				transactions_defs.push(TransactionBuildParams { account: account.clone(), nonce });
				nonce = nonce.map(|n| n + 1);
			}
			build_transactions(builder, sink.clone(), unwatched, transactions_defs, recipe).await
		},
		SendingScenario::FromManyAccounts { start_id, last_id, from, count } => {
			let mut transactions_defs = vec![];

			for account in *start_id..=*last_id {
				let mut nonce = *from;
				for _ in 0..*count {
					transactions_defs
						.push(TransactionBuildParams { account: account.to_string(), nonce });
					nonce = nonce.map(|n| n + 1);
				}
			}

			build_transactions(builder, sink.clone(), unwatched, transactions_defs, recipe).await
		},
	};

	let transactions = transactions.into_iter().rev().collect();
	let (queue, queue_task) = DefaultResubmissionQueue::new();
	let (stop_runner_tx, mut runner) = Runner::<
		DefaultTxTask<T>,
		S,
		DefaultResubmissionQueue<DefaultTxTask<T>>,
	>::new(send_threshold, sink, transactions, queue);

	ctrlc::set_handler(move || {
		block_on(stop_runner_tx.send(())).expect("Could not send signal on channel.")
	})
	.expect("Error setting Ctrl-C handler");
	join(queue_task, runner.run_poc2()).await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	init_logger();

	let cli = cli::Cli::parse();

	match &cli.command {
		CliCommand::Tx {
			chain,
			ws,
			unwatched,
			block_monitor,
			mortal,
			log_file,
			scenario,
			send_threshold,
			remark,
		} => {
			let recipe = remark.map_or_else(TransactionRecipe::transfer, TransactionRecipe::remark);
			match chain {
				ChainType::Fake => {
					let sink = FakeTransactionSink::new();
					let builder = FakeTransactionBuilder::new();
					execute_scenario(
						sink,
						builder,
						scenario,
						*unwatched,
						*send_threshold as usize,
						&recipe,
					)
					.await;
				},
				ChainType::Eth => {
					let transaction_monitor =
						if *block_monitor { Some(BlockMonitor::new(ws).await) } else { None };

					let def = scenario.get_accounts_description();
					let sink = EthTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						def,
						generate_ecdsa_keypair,
						transaction_monitor,
					)
					.await;
					let builder = EthTransactionBuilder::new();
					execute_scenario(
						sink,
						builder,
						scenario,
						*unwatched,
						*send_threshold as usize,
						&recipe,
					)
					.await;
				},
				ChainType::Sub => {
					let transaction_monitor =
						if *block_monitor { Some(BlockMonitor::new(ws).await) } else { None };
					let def = scenario.get_accounts_description();
					let sink = SubstrateTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						def,
						generate_sr25519_keypair,
						transaction_monitor,
					)
					.await;
					let builder = SubstrateTransactionBuilder::new();
					execute_scenario(
						sink,
						builder,
						scenario,
						*unwatched,
						*send_threshold as usize,
						&recipe,
					)
					.await;
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
						None,
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
						None,
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
			let api = subxt::OnlineClient::<EthRuntimeConfig>::from_insecure_url(ws).await?;
			let runtime_apis = api.runtime_api().at_latest().await?;
			let (_, meta): (Compact<u32>, RuntimeMetadataPrefixed) =
				runtime_apis.call_raw("Metadata_metadata", None).await?;
			println!("{meta:#?}");
		},
		CliCommand::BlockMonitor { chain, ws } => {
			match chain {
				ChainType::Sub => {
					let block_monitor = BlockMonitor::<PolkadotConfig>::new(ws).await;
					async {
						loop {
							tokio::time::sleep(Duration::from_secs(10)).await
						}
					}
					.await;
				},
				ChainType::Eth => {
					let block_monitor = BlockMonitor::<EthRuntimeConfig>::new(ws).await;
					async {
						loop {
							tokio::time::sleep(Duration::from_secs(10)).await
						}
					}
					.await;
				},
				ChainType::Fake => {
					unimplemented!()
				},
			};
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
		CliCommand::GenerateEndowedAccounts {
			chain,
			start_id,
			last_id,
			balance,
			out_file_name,
			chain_spec,
		} => {
			let accounts_description = AccountsDescription::Derived(*start_id..last_id + 1);
			let funded_accounts = match chain {
				ChainType::Sub => {
					let accounts = subxt_transaction::derive_accounts::<PolkadotConfig, _, _>(
						accounts_description.clone(),
						subxt_transaction::SENDER_SEED,
						generate_sr25519_keypair,
					);
					accounts
						.values()
						.map(|keypair| {
							serde_json::json!((
								<PolkadotConfig as subxt::Config>::AccountId::from(
									keypair.0.clone().public_key()
								),
								balance,
							))
						})
						.collect::<Vec<_>>()
				},
				ChainType::Eth => {
					let accounts = subxt_transaction::derive_accounts::<EthRuntimeConfig, _, _>(
						accounts_description.clone(),
						subxt_transaction::SENDER_SEED,
						generate_ecdsa_keypair,
					);
					accounts
						.values()
						.map(|keypair| {
							serde_json::json!((
								"0x".to_string() + &hex::encode(keypair.0.clone().account_id()),
								balance,
							))
						})
						.collect::<Vec<_>>()
				},
				ChainType::Fake => Default::default(),
			};

			if let Some(chain_spec) = chain_spec {
				let file = File::open(chain_spec)?;
				let reader = BufReader::new(file);
				let mut chain_spec: serde_json::Value = serde_json::from_reader(reader)?;

				if let Some(balances) = chain_spec["genesis"]["runtimeGenesis"]["patch"]["balances"]
					["balances"]
					.as_array_mut()
				{
					balances.extend(funded_accounts);
				} else {
					return Err("Balances array not found in provided chain-spec".into());
				}

				fs::write(
					out_file_name,
					serde_json::to_string_pretty(&chain_spec)
						.map_err(|e| format!("to pretty failed: {e}"))?,
				)
				.map_err(|err| err.to_string())?;
			} else {
				let json_object = serde_json::json!({"balances":{"balances":funded_accounts}});
				fs::write(out_file_name, serde_json::to_string_pretty(&json_object)?.as_bytes())?;
			}
		},
	};
	Ok(())
}
