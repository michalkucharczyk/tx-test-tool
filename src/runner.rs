use crate::{
	execution_log::{
		journal::Journal, make_stats, Counters, DefaultExecutionLog, ExecutionEvent, ExecutionLog,
		Logs,
	},
	fake_transaction::FakeTransaction,
	fake_transaction_sink::FakeTransactionSink,
	resubmission::{DefaultResubmissionQueue, NeedsResubmit, ResubmissionQueue, ResubmitReason},
	transaction::{
		ResubmitHandler, Transaction, TransactionStatus, TransactionStatusIsTerminal,
		TransactionsSink,
	},
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use std::{
	cmp::min,
	fmt::Display,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};
use subxt::config::BlockHash;
use tokio::{
	select,
	sync::mpsc::{channel, Receiver, Sender},
	task::yield_now,
};
use tracing::{debug, info, trace};

const LOG_TARGET: &str = "runner";

pub enum ExecutionResult<T: TxTask> {
	NeedsResubmit(ResubmitReason, T),
	Error(TxTashHash<T>),
	Done(TxTashHash<T>),
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

#[async_trait]
pub trait TxTask: Send + Sync + Sized {
	type Transaction: Transaction + ResubmitHandler;

	fn tx(&self) -> &Self::Transaction;
	fn is_watched(&self) -> bool;

	async fn send_watched_tx(
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self>;

	async fn send_tx(
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self>;

	async fn execute(
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self>;

	fn handle_resubmit_request(self) -> Option<Self>;
}

pub type TxTashHash<T> = <<T as TxTask>::Transaction as Transaction>::HashType;

pub struct DefaultTxTask<T>
where
	T: Transaction,
	<T as Transaction>::HashType: 'static,
{
	tx: T,
	watched: bool,
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
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self> {
		log.push_event(ExecutionEvent::sent());
		match rpc.submit_and_watch(self.tx()).await {
			Ok(mut stream) => {
				log.push_event(ExecutionEvent::submit_and_watch_result(Ok(())));
				while let Some(status) = stream.next().await {
					// yield_now().await;
					debug!(target:LOG_TARGET,hash=?self.tx().hash(),"status: {status:?}");
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
				ExecutionResult::Error(self.tx().hash())
			},
			Err(e) => {
				debug!(hash=?self.tx().hash(),"error: {e:?}");
				let result = if let Some(reason) = e.needs_resubmission() {
					ExecutionResult::NeedsResubmit(reason, self)
				} else {
					ExecutionResult::Error(self.tx().hash())
				};
				log.push_event(ExecutionEvent::submit_and_watch_result(Err(e)));
				result
			},
		}
	}

	async fn send_tx(
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self> {
		log.push_event(ExecutionEvent::sent());
		match rpc.submit(self.tx()).await {
			Ok(_) => {
				log.push_event(ExecutionEvent::submit_result(Ok(())));
				//todo: block monitor await here (with some global (cli-provided) timeout)
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
		self: Self,
		log: Arc<dyn ExecutionLog<HashType = TxTashHash<Self>>>,
		rpc: Arc<dyn TransactionsSink<TxTashHash<Self>>>,
	) -> ExecutionResult<Self> {
		if self.is_watched() {
			self.send_watched_tx(log, rpc).await
		} else {
			self.send_tx(log, rpc).await
		}
	}
}

// pub trait TransactionsStore<H: BlockHash, T: Transaction<H>>: Sync {
//     fn ready(&self) -> impl IntoIterator<Item = >;
// }

trait TxTaskStore {
	type HashType: BlockHash;
	type Transaction: Transaction<HashType = Self::HashType>;
	fn pop() -> Option<Self::Transaction>;
}

pub type FakeTxTask = DefaultTxTask<FakeTransaction>;

impl<T: Transaction> DefaultTxTask<T> {
	pub fn new_watched(tx: T) -> Self {
		Self { tx, watched: true }
	}
	pub fn new_unwatched(tx: T) -> Self {
		Self { tx, watched: false }
	}
}

pub struct Runner<T: TxTask, Sink: TransactionsSink<TxTashHash<T>>, Queue: ResubmissionQueue<T>> {
	initial_tasks: usize,
	logs: Logs<T>,
	transactions: Vec<T>,
	done: Vec<TxTashHash<T>>,
	errors: Vec<TxTashHash<T>>,
	rpc: Arc<Sink>,
	resubmission_queue: Queue,
	stop_rx: Receiver<()>,
	event_counters: Arc<Counters>,
	last_displayed: Option<Instant>,
}

type FakeTxRunner = Runner<FakeTxTask, FakeTransactionSink, DefaultResubmissionQueue<FakeTxTask>>;

impl<T: TxTask + 'static, Sink, Queue: ResubmissionQueue<T>> Runner<T, Sink, Queue>
where
	Sink: TransactionsSink<TxTashHash<T>> + 'static,
	TxTashHash<T>: 'static,
{
	pub fn new(
		initial_tasks: usize,
		rpc: Sink,
		transactions: Vec<T>,
		resubmission_queue: Queue,
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
				transactions: transactions.into(),
				rpc: rpc.into(),
				resubmission_queue,
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
		let r = self.resubmission_queue.pop().or_else(|| self.transactions.pop());
		trace!(target:LOG_TARGET, "after pop");
		r
	}

	pub async fn consume_pending(
		&mut self,
		workers: &mut FuturesUnordered<Pin<Box<dyn Future<Output = ExecutionResult<T>> + Send>>>,
		single: bool,
	) {
		if let Some(last_displayed) = self.last_displayed {
			if last_displayed.elapsed() < Duration::from_millis(1000) {
				return;
			}
		}
		self.last_displayed = Some(Instant::now());
		let current_count = self.rpc.count().await;
		let gap = self
			.initial_tasks
			.saturating_sub(current_count)
			.saturating_sub(self.event_counters.buffered());
		let workers_len = workers.len();
		let mut pushed = 0;
		let to_consume = if single { min(1, gap) } else { gap };
		let counters_displayed = format!("{}", self.event_counters);
		for _ in 0..to_consume {
			let task = self.pop();
			if let Some(task) = task {
				let hash = task.tx().hash();
				let log = self.logs[&task.tx().hash()].clone();
				log.push_event(ExecutionEvent::popped());
				trace!(target:LOG_TARGET,hash = ?hash, workers_len=workers.len(), "before push");
				workers.push(task.execute(log, self.rpc.clone()));
				pushed = pushed + 1;
				trace!(target:LOG_TARGET,hash = ?hash, workers_len=workers.len(), "after push");
			} else {
				break;
			};
		}

		info!(
			workers_len,
			gap,
			current_count,
			initial_task = self.initial_tasks,
			pushed,
			"consume pending {}",
			counters_displayed,
		);
	}

	pub async fn run_poc2(&mut self) {
		let mut workers = FuturesUnordered::new();

		let mut i = 0;
		for _ in 0..self.initial_tasks {
			if let Some(t) = self.transactions.pop() {
				let t = Box::new(t);
				let log = self.logs[&t.tx().hash()].clone();
				log.push_event(ExecutionEvent::popped());
				workers.push(t.execute(log, self.rpc.clone()));
				i = i + 1;
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
							if self.resubmission_queue.is_empty().await {
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

		// info!("logs {:#?}", self.logs);
		Journal::<T>::save_logs(self.logs.clone());
		make_stats(self.logs.values().cloned(), false)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		fake_transaction::FakeTransaction,
		fake_transaction_sink::FakeTransactionSink,
		init_logger,
		runner::{DefaultResubmissionQueue, FakeTxTask},
		subxt_api_connector,
		subxt_transaction::{
			EthRuntimeConfig, EthTransaction, EthTransactionsSink, SubxtTransactionsSink,
		},
		transaction::AccountMetadata,
	};
	use futures::future::join;
	use subxt::{
		config::substrate::SubstrateExtrinsicParamsBuilder as Params, dynamic::Value, OnlineClient,
	};
	use subxt_signer::eth::dev;
	use tracing::trace;

	#[tokio::test]
	async fn oh_god() {
		init_logger();

		let rpc = FakeTransactionSink::new();
		let mut transactions = (0..10)
			.map(|i| FakeTxTask::new_watched(FakeTransaction::new_finalizable_quick(i)))
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		transactions.push(FakeTxTask::new_watched(FakeTransaction::new_droppable(11u32, 300)));
		transactions.push(FakeTxTask::new_watched(FakeTransaction::new_invalid(11u32, 300)));
		transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_invalid(11u32, 300)));
		transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_error(12u32, 300)));

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_c, mut r) = Runner::<
			DefaultTxTask<FakeTransaction>,
			FakeTransactionSink,
			DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
		>::new(5, rpc, transactions, queue);
		join(queue_task, r.run_poc2()).await;
	}

	type EthTestTxTask = DefaultTxTask<EthTransaction>;

	fn make_eth_test_transaction(
		api: &OnlineClient<EthRuntimeConfig>,
		nonce: u64,
	) -> EthTransaction {
		let alith = dev::alith();
		let baltathar = dev::baltathar();

		let nonce = nonce;
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
				Value::unnamed_composite(vec![Value::from_bytes(alith.account_id())]),
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

	#[tokio::test]
	async fn oh_god2() {
		init_logger();

		// let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
		//     .await
		//     .unwrap();
		let api = subxt_api_connector::connect("ws://127.0.0.1:9933").await.unwrap();

		let rpc = EthTransactionsSink::new().await;

		let transactions = (0..3000)
			.map(|i| EthTestTxTask::new_watched(make_eth_test_transaction(&api, i + 0)))
			.rev()
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_c, mut r) = Runner::<
			DefaultTxTask<EthTransaction>,
			EthTransactionsSink,
			DefaultResubmissionQueue<DefaultTxTask<EthTransaction>>,
		>::new(10_000, rpc, transactions, queue);
		join(queue_task, r.run_poc2()).await;
	}

	#[tokio::test]
	async fn resubmit() {
		init_logger();

		let rpc = FakeTransactionSink::new();
		let transactions = (0..100000)
			.map(|i| FakeTxTask::new_watched(FakeTransaction::new_droppable_2nd_success(i, 0)))
			// .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
			.collect::<Vec<_>>();

		let (queue, queue_task) = DefaultResubmissionQueue::new();

		let (_, mut r) = Runner::<
			DefaultTxTask<FakeTransaction>,
			FakeTransactionSink,
			DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
		>::new(100000, rpc, transactions, queue);
		join(queue_task, r.run_poc2()).await;
	}

	#[tokio::test]
	async fn read_json() {
		init_logger();
		let logs = Journal::<DefaultTxTask<FakeTransaction>>::load_logs("out_20240801_164155.json");
		make_stats(logs.values().cloned(), true);
	}
}
