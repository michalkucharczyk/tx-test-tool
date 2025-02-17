use crate::{
	execution_log::{
		journal::Journal, make_stats, Counters, DefaultExecutionLog, ExecutionEvent, ExecutionLog,
		Logs, STAT_TARGET,
	},
	resubmission::{NeedsResubmit, ResubmissionQueue, ResubmitReason},
	transaction::{ResubmitHandler, Transaction, TransactionStatusIsDone, TransactionsSink},
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use std::{
	cmp::min,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};
use subxt::config::BlockHash;
use tokio::{
	select,
	sync::mpsc::{channel, Receiver, Sender},
};
use tracing::{debug, info, trace};

const LOG_TARGET: &str = "runner";

/// Transaction hash type.
pub type TxTaskHash<T> = <<T as TxTask>::Transaction as Transaction>::HashType;

/// Provides a transaction execution result.
pub enum ExecutionResult<T: TxTask> {
	NeedsResubmit(ResubmitReason, T),
	Error(TxTaskHash<T>),
	Done(TxTaskHash<T>),
}

impl<T: TxTask> std::fmt::Debug for ExecutionResult<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::NeedsResubmit(r, t) => write!(f, "NeedsResubmit {r:?} {:?}", t.tx().hash()),
			Self::Error(h) => write!(f, "Error {h:?}"),
			Self::Done(h) => write!(f, "Done {h:?}"),
		}
	}
}

/// Interface for tasks that monitor transaction execution.
#[async_trait]
pub trait TxTask: Send + Sync + Sized + std::fmt::Debug {
	type Transaction: Transaction + ResubmitHandler;

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

	fn handle_resubmit_request(self) -> Option<Self>;
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
impl<H: BlockHash, T: Transaction<HashType = H> + ResubmitHandler + Send> TxTask
	for DefaultTxTask<T>
{
	type Transaction = T;

	fn tx(&self) -> &T {
		&self.tx
	}

	fn is_watched(&self) -> bool {
		self.watched
	}

	fn handle_resubmit_request(mut self) -> Option<Self> {
		if let Some(new_tx) = self.tx.handle_resubmit_request() {
			self.tx = new_tx;
			Some(self)
		} else {
			None
		}
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
					} else if let Some(reason) = status.needs_resubmission() {
						return ExecutionResult::NeedsResubmit(reason, self);
					} else if status.is_error() {
						return ExecutionResult::Error(self.tx().hash());
					} else {
						continue;
					}
				}
				//shall not happen to be here
				panic!();
				// ExecutionResult::Error(self.tx().hash())
			},
			Err(e) => {
				let (hash, nonce) = (self.tx().hash(), self.tx().nonce());
				let result = if let Some(reason) = e.needs_resubmission() {
					ExecutionResult::NeedsResubmit(reason, self)
				} else {
					info!(nonce=?self.tx().nonce(),"submit_and_watch: error: {e:?}");
					ExecutionResult::Error(self.tx().hash())
				};
				// debug!(?hash, nonce, "error: {e:?} {result:?}");
				if !matches!(result, ExecutionResult::NeedsResubmit(ResubmitReason::Dropped, _)) {
					info!(?hash, nonce, "error: {e:?} {result:?}");
				}
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
				log.push_event(ExecutionEvent::submit_result(Ok(())));
				//todo: block monitor await here (with some global (cli-provided) timeout)
				if let Some(monitor) = rpc.transaction_monitor() {
					let hash = monitor.wait(self.tx().hash()).await;
					log.push_event(ExecutionEvent::finalized_monitor(hash));
				}
				ExecutionResult::Done(self.tx().hash())
			},
			Err(e) => {
				let result = if let Some(reason) = e.needs_resubmission() {
					ExecutionResult::NeedsResubmit(reason, self)
				} else {
					ExecutionResult::Error(self.tx().hash())
				};
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
pub struct Runner<T: TxTask, Sink: TransactionsSink<TxTaskHash<T>>, Queue: ResubmissionQueue<T>> {
	initial_tasks: usize,
	logs: Logs<T>,
	transactions: Vec<T>,
	done: Vec<TxTaskHash<T>>,
	errors: Vec<TxTaskHash<T>>,
	rpc: Arc<Sink>,
	resubmission_queue: Queue,
	stop_rx: Receiver<()>,
	event_counters: Arc<Counters>,
	last_displayed: Option<Instant>,
}

impl<T: TxTask + 'static, Sink, Queue: ResubmissionQueue<T>> Runner<T, Sink, Queue>
where
	Sink: TransactionsSink<TxTaskHash<T>> + 'static,
	TxTaskHash<T>: 'static,
{
	/// Instantiates a new transactions [`Runner`].
	pub fn new(
		initial_tasks: usize,
		rpc: Sink,
		transactions: Vec<T>,
		queue: Queue,
	) -> (Sender<()>, Self) {
		let event_counters = Arc::from(Counters::default());
		let logs = transactions
			.iter()
			.map(|t| {
				(
					t.tx().hash(),
					Arc::new(DefaultExecutionLog::new_with_tx(t.tx(), event_counters.clone())),
				)
			})
			.collect();

		let (tx, rx) = channel(1);
		(
			tx,
			Self {
				initial_tasks,
				logs,
				transactions,
				rpc: rpc.into(),
				resubmission_queue: queue,
				done: Default::default(),
				errors: Default::default(),
				stop_rx: rx,
				event_counters,
				last_displayed: None,
			},
		)
	}

	fn pop(&mut self) -> Option<T> {
		trace!(target:LOG_TARGET, "before pop");
		let r = self.resubmission_queue.pop().or_else(|| {
			if self.resubmission_queue.is_empty() {
				self.transactions.pop()
			} else {
				None
			}
		});
		trace!(target:LOG_TARGET, "after pop {}", r.is_some());
		r
	}

	async fn consume_pending(
		&mut self,
		workers: &mut FuturesUnordered<Pin<Box<dyn Future<Output = ExecutionResult<T>> + Send>>>,
		single: bool,
	) {
		let current_count = self.rpc.count().await;
		let gap = self
			.initial_tasks
			.saturating_sub(current_count)
			.saturating_sub(self.event_counters.buffered());
		let workers_len = workers.len();
		let mut pushed = 0;
		let to_consume = if single { min(1, gap) } else { gap };
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

			info!(
				workers_len,
				gap,
				current_count,
				initial_task = self.initial_tasks,
				pushed,
				min_nonce_sent,
				"consume pending {}",
				counters_displayed,
			);
		};
	}

	/// Drives the runner to completion.
	pub async fn run(&mut self) -> Logs<T> {
		let start = Instant::now();
		let original_transactions_count = self.transactions.len();
		let mut workers = FuturesUnordered::new();

		for _ in 0..self.initial_tasks {
			if let Some(t) = self.transactions.pop() {
				let t = Box::new(t);
				let log = self.logs[&t.tx().hash()].clone();
				log.push_event(ExecutionEvent::popped());
				workers.push(t.execute(log, self.rpc.clone()));
			} else {
				break;
			}
		}

		loop {
			select! {
				_ = tokio::time::sleep(Duration::from_millis(3000)) => {
					self.consume_pending(&mut workers, false).await;
				}
				_ = self.stop_rx.recv() => {
					self.resubmission_queue.forced_terminate();
					info!("received termination request");
					break;
				}
				done = workers.next() => {
					match done {
						Some(result) => {
							debug!(target:LOG_TARGET,?result, workers_len=workers.len(), "FINISHED");
							self.consume_pending(&mut workers, false).await;

							match result {
								ExecutionResult::NeedsResubmit(reason, t) => {
								let log = self.logs[&t.tx().hash()].clone();
									log.push_event(ExecutionEvent::resubmitted());
									self.resubmission_queue.resubmit(t, reason).await;
								},
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
							debug!(target:LOG_TARGET,"all futures done");
							tokio::time::sleep(Duration::from_millis(100)).await;
							if self.resubmission_queue.is_empty() {
								self.resubmission_queue.terminate();
								debug!(target:LOG_TARGET,"done");
								break;
							}
							self.consume_pending(&mut workers, false).await;
						}
					}
				}
			}
		}

		Journal::<T>::save_logs(self.logs.clone());
		info!(target: STAT_TARGET, total_duration = ?start.elapsed(), ?original_transactions_count);
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
		init_logger,
		resubmission::DefaultResubmissionQueue,
		subxt_api_connector,
		subxt_transaction::{EthRuntimeConfig, EthTransaction, EthTransactionsSink},
		transaction::AccountMetadata,
	};
	use futures::future::join;
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

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_c, mut r) = Runner::<
			DefaultTxTask<FakeTransaction>,
			FakeTransactionsSink,
			DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
		>::new(5, rpc, transactions, queue);
		join(queue_task, r.run()).await;
	}

	type EthTestTxTask = DefaultTxTask<EthTransaction>;

	fn make_eth_test_transaction(
		api: &OnlineClient<EthRuntimeConfig>,
		nonce: u64,
	) -> EthTransaction {
		let alith = dev::alith();
		let baltathar = dev::baltathar();

		let tx_params = Params::new().nonce(nonce).build();

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
			api.tx().create_signed_offline(&tx_call, &baltathar, tx_params).unwrap(),
			nonce as u128,
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
		let api = subxt_api_connector::connect("ws://127.0.0.1:9933").await.unwrap();

		let rpc = EthTransactionsSink::new().await;

		let transactions = (0..3000)
			.map(|i| EthTestTxTask::new_watched(make_eth_test_transaction(&api, i)))
			.rev()
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_c, mut r) = Runner::<
			DefaultTxTask<EthTransaction>,
			EthTransactionsSink,
			DefaultResubmissionQueue<DefaultTxTask<EthTransaction>>,
		>::new(10_000, rpc, transactions, queue);
		join(queue_task, r.run()).await;
	}

	#[tokio::test]
	async fn resubmit() {
		init_logger();

		let rpc = FakeTransactionsSink::default();
		let transactions = (0..100000)
			.map(|i| FakeTxTask::new_watched(FakeTransaction::new_droppable_2nd_success(i, 0, 0)))
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_, mut r) = Runner::<
			DefaultTxTask<FakeTransaction>,
			FakeTransactionsSink,
			DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
		>::new(100000, rpc, transactions, queue);

		join(queue_task, r.run()).await;
	}

	#[tokio::test]
	async fn read_json() {
		init_logger();
		let logs =
			Journal::<DefaultTxTask<FakeTransaction>>::load_logs("tests/out_20250206_151339.json");
		make_stats(logs.values().cloned(), true);
	}
}
