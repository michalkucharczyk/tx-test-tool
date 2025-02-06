use crate::{runner::TxTask, transaction::Transaction};
use std::sync::Arc;

use crate::{
	fake_transaction::{FakeHash, FakeTransaction},
	fake_transaction_sink::FakeTransactionSink,
	resubmission::DefaultResubmissionQueue,
	runner::{DefaultTxTask, Runner},
	subxt_transaction::{
		build_eth_tx_payload, build_substrate_tx_payload, build_subxt_tx, EthRuntimeConfig,
		EthTransaction, EthTransactionsSink, HashOf, SubstrateTransaction,
		SubstrateTransactionsSink,
	},
	transaction::{ResubmitHandler, SendingScenario, TransactionRecipe, TransactionsSink},
};
use async_trait::async_trait;
use futures::{executor::block_on, future::join};
use subxt::{config::BlockHash, PolkadotConfig};

pub mod block_monitor;
pub mod cli;
pub mod error;
pub mod execution_log;
pub mod fake_transaction;
pub mod fake_transaction_sink;
pub mod resubmission;
pub mod runner;
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
	type Sink = FakeTransactionSink;
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

pub async fn execute_scenario<H, T, S, B>(
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
		SendingScenario::OneShot { account, nonce } => {
			vec![builder.build_transaction(account, nonce, &sink, unwatched, recipe).await]
		},
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
	let var_name = Runner::<DefaultTxTask<T>, S, DefaultResubmissionQueue<DefaultTxTask<T>>>::new(
		send_threshold,
		sink,
		transactions,
		queue,
	);
	let (stop_runner_tx, mut runner) = var_name;

	ctrlc::set_handler(move || {
		block_on(stop_runner_tx.send(())).expect("Could not send signal on channel.")
	})
	.expect("Error setting Ctrl-C handler");
	join(queue_task, runner.run_poc2()).await;
}
