// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use crate::{
	error::Error,
	execution_log::{
		journal::Journal, make_stats, Counters, ExecutionEvent, ExecutionLog, Logs,
		TransactionExecutionLog, STAT_TARGET,
	},
	transaction::{Transaction, TransactionStatusIsDone, TransactionsSink},
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use std::{
	path::Path,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant, SystemTime},
};
use subxt::config::BlockHash;
use tokio::{
	select,
	sync::mpsc::{channel, Receiver, Sender},
};
use tracing::{debug, info, instrument, trace, warn, Span};

const LOG_TARGET: &str = "runner";

/// Transaction hash type.
pub type TxTaskHash<T> = <<T as TxTask>::Transaction as Transaction>::HashType;

/// Provides a transaction execution result.
pub enum ExecutionResult<T: TxTask> {
	Error(TxTaskHash<T>),
	Done(TxTaskHash<T>),
}

impl<T: TxTask> std::fmt::Debug for ExecutionResult<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Error(h) => write!(f, "Error {h:?}"),
			Self::Done(h) => write!(f, "Done {h:?}"),
		}
	}
}

/// Interface for tasks that monitor transaction execution.
#[async_trait]
pub trait TxTask: Send + Sync + Sized + std::fmt::Debug {
	type Transaction: Transaction;

	fn tx(&self) -> &Self::Transaction;
	fn is_watched(&self) -> bool;

	async fn send_watched_tx(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self>;

	async fn send_tx(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self>;

	async fn execute(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self>;
}

/// Holds the logic for a transaction submission.
pub struct DefaultTxTask<T>
where
	T: Transaction,
	<T as Transaction>::HashType: 'static,
{
	tx: T,
	watched: bool,
}

impl<T> std::fmt::Debug for DefaultTxTask<T>
where
	T: Transaction,
{
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("tx").field("nonce", &self.tx.nonce()).finish()
	}
}

#[async_trait]
impl<H: BlockHash, T: Transaction<HashType = H> + Send> TxTask for DefaultTxTask<T> {
	type Transaction = T;

	fn tx(&self) -> &T {
		&self.tx
	}

	fn is_watched(&self) -> bool {
		self.watched
	}

	async fn send_watched_tx(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self> {
		log.push_event(ExecutionEvent::sent());
		match rpc.submit_and_watch(self.tx()).await {
			Ok(mut stream) => {
				log.push_event(ExecutionEvent::submit_and_watch_result(Ok(())));
				while let Some(status) = stream.next().await {
					debug!(target:LOG_TARGET,tx=?self,"status: {status:?}");
					log.push_event(status.clone().into());
					if status.is_finalized() {
						return ExecutionResult::Done(self.tx().hash());
					} else if status.is_error() {
						return ExecutionResult::Error(self.tx().hash());
					} else {
						continue;
					}
				}
				//shall not happen to be here, return error.
				warn!(target:LOG_TARGET,tx=?self,"stream error");
				ExecutionResult::Error(self.tx().hash())
			},
			Err(e) => {
				info!(nonce=?self.tx().nonce(),"submit_and_watch: error: {e:?}");
				let result = ExecutionResult::Error(self.tx().hash());
				// debug!(?hash, nonce, "error: {e:?} {result:?}");
				log.push_event(ExecutionEvent::submit_and_watch_result(Err(e)));
				result
			},
		}
	}

	async fn send_tx(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self> {
		log.push_event(ExecutionEvent::sent());
		match rpc.submit(self.tx()).await {
			Ok(_) => {
				//todo: block monitor await here (with some global (cli-provided) timeout)
				if let Some(monitor) = rpc.transaction_monitor() {
					if let Some(mortality) = self.tx().mortality() {
						tokio::time::timeout(
							// Consider that 10 blocks are worth a minute
							Duration::from_secs((mortality / 10 + 1) * 60),
							monitor.wait(self.tx().hash()),
						)
						.await
						.map(|hash| {
							log.push_event(ExecutionEvent::finalized_monitor(hash));
							ExecutionResult::Done(self.tx().hash())
						})
						.unwrap_or_else(|_| {
							let ev_err = Err(Error::Other(format!(
								"timeout while waiting for mortal tx: {}",
								self.tx().hash()
							)));
							log.push_event(ExecutionEvent::submit_result(ev_err));
							ExecutionResult::Error(self.tx().hash())
						})
					} else {
						// todo: add an else branch where we consider a cli-provided global timeout
						log.push_event(ExecutionEvent::submit_result(Ok(())));
						ExecutionResult::Done(self.tx.hash())
					}
				} else {
					// todo: add an else branch where we consider a cli-provided global timeout
					log.push_event(ExecutionEvent::submit_result(Ok(())));
					ExecutionResult::Done(self.tx.hash())
				}
			},
			Err(e) => {
				let result = ExecutionResult::Error(self.tx().hash());
				log.push_event(ExecutionEvent::submit_result(Err(e)));
				result
			},
		}
	}

	async fn execute(
		self,
		log: Arc<dyn ExecutionLog<HashType = TxTaskHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTaskHash<Self>>>,
	) -> ExecutionResult<Self> {
		if self.is_watched() {
			self.send_watched_tx(log, rpc).await
		} else {
			self.send_tx(log, rpc).await
		}
	}
}

impl<T: Transaction> DefaultTxTask<T> {
	pub fn new_watched(tx: T) -> Self {
		Self { tx, watched: true }
	}
	pub fn new_unwatched(tx: T) -> Self {
		Self { tx, watched: false }
	}
}

/// Holds the logic that handles multiple transactions execution on a specific chain.
pub struct Runner<T: TxTask, Sink: TransactionsSink<TxTaskHash<T>>> {
	send_threshold: usize,
	logs: Logs<T>,
	transactions: Vec<T>,
	done: Vec<TxTaskHash<T>>,
	errors: Vec<TxTaskHash<T>>,
	rpc: Arc<Sink>,
	stop_rx: Receiver<()>,
	timeout: Option<Duration>,
	event_counters: Arc<Counters>,
	last_displayed: Option<Instant>,
	log_file_name: Option<String>,
	base_dir_path: Option<String>,
	executor_id: Option<String>,
}

impl<T: TxTask + 'static, Sink> Runner<T, Sink>
where
	Sink: TransactionsSink<TxTaskHash<T>> + 'static,
	TxTaskHash<T>: 'static,
{
	/// Instantiates a new transactions [`Runner`].
	pub fn new(
		send_threshold: usize,
		rpc: Sink,
		transactions: Vec<T>,
		log_file_name: Option<String>,
		base_dir_path: Option<String>,
		executor_id: Option<String>,
		timeout: Option<Duration>,
	) -> (Sender<()>, Self) {
		let event_counters = Arc::from(Counters::default());
		let logs = transactions
			.iter()
			.map(|t| {
				(
					t.tx().hash(),
					Arc::new(TransactionExecutionLog::new_with_tx(t.tx(), event_counters.clone())),
				)
			})
			.collect();

		let (tx, rx) = channel(1);
		(
			tx,
			Self {
				send_threshold,
				logs,
				transactions,
				rpc: rpc.into(),
				done: Default::default(),
				errors: Default::default(),
				stop_rx: rx,
				event_counters,
				last_displayed: None,
				log_file_name,
				base_dir_path,
				executor_id,
				timeout,
			},
		)
	}

	fn pop(&mut self) -> Option<T> {
		trace!(target:LOG_TARGET, "before pop");
		let r = self.transactions.pop();
		trace!(target:LOG_TARGET, "after pop {}", r.is_some());
		r
	}

	async fn consume_pending(
		&mut self,
		workers: &mut FuturesUnordered<Pin<Box<dyn Future<Output = ExecutionResult<T>> + Send>>>,
	) {
		let (to_consume, current_count) = if self.send_threshold == usize::MAX {
			(usize::MAX, None)
		} else {
			let current_count = self.rpc.pending_extrinsics().await;
			(
				self.send_threshold
					.saturating_sub(current_count)
					.saturating_sub(self.event_counters.buffered()),
				Some(current_count),
			)
		};
		let mut pushed = 0;
		let counters_displayed = format!("{}", self.event_counters);
		let mut nonces = vec![];
		for _ in 0..to_consume {
			let task = self.pop();
			if let Some(task) = task {
				nonces.push(task.tx().nonce());
				let log = self.logs[&task.tx().hash()].clone();
				log.push_event(ExecutionEvent::popped());
				trace!(target:LOG_TARGET,task = ?&task, workers_len=workers.len(), "before push");
				workers.push(task.execute(log, self.rpc.clone()));
				pushed += 1;
				trace!(target:LOG_TARGET,workers_len=workers.len(), "after push");
			} else {
				break;
			};
		}
		let min_nonce_sent = nonces.iter().min();

		let display = if let Some(last_displayed) = self.last_displayed {
			last_displayed.elapsed() > Duration::from_millis(200)
		} else {
			true
		};

		if display {
			self.last_displayed = Some(Instant::now());
			if let Some(current_count) = current_count {
				info!(
					current_count,
					pushed, min_nonce_sent, "consume pending {}", counters_displayed,
				);
			} else {
				info!(pushed, min_nonce_sent, "consume pending {}", counters_displayed,);
			}
		};
	}

	fn log_file_path(&self) -> String {
		let datetime: chrono::DateTime<chrono::Local> = SystemTime::now().into();
		let formatted_date = datetime.format("%Y%m%d_%H%M%S");
		let default_file_name = self
			.executor_id
			.as_ref()
			.map(|id| format!("ttxt_{}_{}.json", id, formatted_date))
			.unwrap_or(format!("ttxt_{}.json", formatted_date));
		self.base_dir_path
			.as_ref()
			.map(|basedir| {
				let filename = self
					.log_file_name
					.as_ref()
					.map(|filename| format!("{basedir}/{filename}_{}", formatted_date))
					.unwrap_or(format!("{basedir}/{default_file_name}"));
				filename
			})
			.unwrap_or(default_file_name)
	}

	/// Drives the runner to completion.
	#[instrument(skip(self), fields(id = tracing::field::Empty))]
	pub async fn run(&mut self) -> Logs<T> {
		let span = Span::current();
		if let Some(id) = &self.executor_id {
			span.record("id", id);
		}

		let start = Instant::now();
		let original_transactions_count = self.transactions.len();
		let mut workers = FuturesUnordered::new();

		for _ in 0..self.send_threshold {
			if let Some(t) = self.transactions.pop() {
				let t = Box::new(t);
				let log = self.logs[&t.tx().hash()].clone();
				log.push_event(ExecutionEvent::popped());
				workers.push(t.execute(log, self.rpc.clone()));
			} else {
				break;
			}
		}

		let mut timeout = self.timeout.unwrap_or(Duration::from_secs(u64::MAX));
		let mut timeout_reached = false;
		loop {
			let iteration_start = Instant::now();
			select! {
				_ = tokio::time::sleep(timeout) => {
					timeout_reached = true;
					break;
				}
				_ = tokio::time::sleep(Duration::from_millis(3000)) => {
					self.consume_pending(&mut workers).await;
				}
				_ = self.stop_rx.recv() => {
					info!("received termination request");
					break;
				}
				done = workers.next() => {
					match done {
						Some(result) => {
							debug!(target:LOG_TARGET,?result, workers_len=workers.len(), "FINISHED");
							self.consume_pending(&mut workers).await;

							match result {
								ExecutionResult::Done(hash) => {
									self.done.push(hash)
								},
								ExecutionResult::Error(hash) => {
									self.errors.push(hash)
								}

							}

							trace!(target:LOG_TARGET, "after match");
						}
						None => {
							info!(target:LOG_TARGET, transactions_len = self.transactions.len(), "all futures done ");
							// tokio::time::sleep(Duration::from_millis(100)).await;
							// self.consume_pending(&mut workers).await;
							break;
						}
					}
				}
			}

			// Adjust timeout based on how long the iteration took.
			timeout = timeout.saturating_sub(iteration_start.elapsed());
		}

		Journal::<T>::save_logs(self.logs.clone(), Path::new(self.log_file_path().as_str()));
		info!(target: STAT_TARGET, total_duration = ?start.elapsed(), ?original_transactions_count, ?timeout_reached);
		make_stats(self.logs.values().cloned(), false);
		self.logs.clone()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		fake_transaction::FakeTransaction,
		fake_transaction_sink::FakeTransactionsSink,
		init_logger, subxt_api_connector,
		subxt_transaction::{EthRuntimeConfig, EthTransaction, EthTransactionsSink},
		transaction::AccountMetadata,
	};
	use subxt::{
		config::substrate::SubstrateExtrinsicParamsBuilder as Params, dynamic::Value, OnlineClient,
	};
	use subxt_signer::eth::dev;
	use tracing::trace;

	pub type FakeTxTask = DefaultTxTask<FakeTransaction>;

	#[tokio::test]
	async fn oh_god() {
		init_logger();

		let rpc = FakeTransactionsSink::default();
		let mut transactions = (0..10)
			.map(|i| FakeTxTask::new_watched(FakeTransaction::new_finalizable_quick(i, 0)))
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		transactions.push(FakeTxTask::new_watched(FakeTransaction::new_droppable(11u32, 0, 300)));
		transactions.push(FakeTxTask::new_watched(FakeTransaction::new_invalid(11u32, 0, 300)));
		transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_invalid(11u32, 0, 300)));
		transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_error(12u32, 0, 300)));

		let (_c, mut r) = Runner::<DefaultTxTask<FakeTransaction>, FakeTransactionsSink>::new(
			5,
			rpc,
			transactions,
			None,
			None,
			None,
			None,
		);
		r.run().await;
	}

	type EthTestTxTask = DefaultTxTask<EthTransaction>;

	fn make_eth_test_transaction(
		api: &OnlineClient<EthRuntimeConfig>,
		mortality: Option<u64>,
		nonce: u64,
	) -> EthTransaction {
		let alith = dev::alith();
		let baltathar = dev::baltathar();

		let mut tx_params = Params::new().nonce(nonce);
		if let Some(mortal) = mortality {
			tx_params = tx_params.mortal(mortal);
		}

		let tx_params = tx_params.build();
		// let tx_call = subxt::dynamic::tx("System", "remark",
		// vec![Value::from_bytes("heeelooo")]);

		let tx_call = subxt::dynamic::tx(
			"Balances",
			"transfer_keep_alive",
			vec![
				// // Substrate:
				// Value::unnamed_variant("Id", [Value::from_bytes(receiver.public())]),
				// Eth:
				Value::unnamed_composite(vec![Value::from_bytes(alith.public_key())]),
				Value::u128(1u32.into()),
			],
		);

		let tx = EthTransaction::new(
			api.tx().create_partial_offline(&tx_call, tx_params).unwrap().sign(&baltathar),
			nonce as u128,
			mortality,
			AccountMetadata::KeyRing("baltathar".to_string()),
		);

		trace!(target:LOG_TARGET,"tx hash: {:?}", tx.hash());

		tx
	}

	// This test needs a network up with a collator responding on 127.0.0.1:9933
	#[ignore]
	#[tokio::test]
	async fn oh_god2() {
		init_logger();

		// let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
		//     .await
		//     .unwrap();
		let api = subxt_api_connector::connect("ws://127.0.0.1:9933", false).await.unwrap();

		let rpc = EthTransactionsSink::new().await;

		let transactions = (0..3000)
			.map(|i| EthTestTxTask::new_watched(make_eth_test_transaction(&api, None, i)))
			.rev()
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (_c, mut r) = Runner::<DefaultTxTask<EthTransaction>, EthTransactionsSink>::new(
			10_000,
			rpc,
			transactions,
			None,
			None,
			None,
			None,
		);
		r.run().await;
	}

	#[tokio::test]
	async fn resubmit() {
		init_logger();

		let rpc = FakeTransactionsSink::default();
		let transactions = (0..100000)
			.map(|i| FakeTxTask::new_watched(FakeTransaction::new_droppable_2nd_success(i, 0, 0)))
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (_, mut r) = Runner::<DefaultTxTask<FakeTransaction>, FakeTransactionsSink>::new(
			100000,
			rpc,
			transactions,
			None,
			None,
			None,
			None,
		);

		r.run().await;
	}

	#[tokio::test]
	async fn read_json() {
		init_logger();
		let logs = Journal::<DefaultTxTask<FakeTransaction>>::load_logs(
			"tests/out_20250206_151339.json",
			&None,
		);
		make_stats(logs.values().cloned(), true);
	}
}
