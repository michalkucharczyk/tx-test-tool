use crate::{
	error::Error,
	runner::TxTask,
	transaction::{Transaction, TransactionStatus},
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, Future, StreamExt};
use jsonrpsee::types::ErrorObject;
use parking_lot::RwLock;
use std::{
	pin::Pin,
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
	time::Duration,
};
use subxt::config::BlockHash;
use tokio::{sync::mpsc, task::yield_now};
use tracing::trace;

const LOG_TARGET: &str = "resubmission";
const DEFAULT_RESUBMIT_DELAY: u64 = 3000;

#[derive(Debug)]
pub enum ResubmitReason {
	Banned,
	Dropped,
	Mortality,
	RpcError,
}

pub trait NeedsResubmit {
	fn needs_resubmission(&self) -> Option<ResubmitReason>;
}

impl<H: BlockHash> NeedsResubmit for TransactionStatus<H> {
	fn needs_resubmission(&self) -> Option<ResubmitReason> {
		matches!(self, TransactionStatus::Dropped(_)).then_some(ResubmitReason::Dropped)
	}
}

impl NeedsResubmit for Error {
	fn needs_resubmission(&self) -> Option<ResubmitReason> {
		if let Error::Subxt(subxt::Error::Rpc(subxt::error::RpcError::ClientError(ref o))) = self {
			if let Some(eo) = o.source() {
				let code = eo.downcast_ref::<ErrorObject>().unwrap().code();
				//polkdot-sdk/substrate/client/rpc-api/author -> POOL_IMMEDIATELY_DROPPED
				if code == 1016 {
					return Some(ResubmitReason::Dropped);
				}
				//polkdot-sdk/substrate/client/rpc-api/author -> POOL_TEMPORARILY_BANNED
				if code == 1012 {
					return Some(ResubmitReason::Banned);
				}
				if code == jsonrpsee::types::error::ErrorCode::InternalError.code() {
					return Some(ResubmitReason::RpcError);
				}
			}
		}
		return None;
	}
}

#[async_trait]
pub trait ResubmissionQueue<T: TxTask> {
	async fn resubmit(&self, hash: T, reason: ResubmitReason);
	fn pop(&self) -> Option<T>;
	fn forced_terminate(&self);
	fn terminate(&self);
	async fn is_empty(&self) -> bool;
}

pub type ResubmittedTxTask<T> = Pin<Box<dyn futures::Future<Output = Option<T>> + Sync + Send>>;
use futures::FutureExt;
pub struct DefaultResubmissionQueue<T: TxTask> {
	ready_queue: Arc<RwLock<Vec<T>>>,
	terminate: Arc<AtomicBool>,
	forced_terminate: Arc<AtomicBool>,
	resubmission_request_tx: mpsc::Sender<ResubmittedTxTask<T>>,
	is_queue_empty: Arc<AtomicBool>,
}

impl<T: TxTask> Clone for DefaultResubmissionQueue<T> {
	fn clone(&self) -> Self {
		Self {
			ready_queue: self.ready_queue.clone(),
			terminate: self.terminate.clone(),
			forced_terminate: self.forced_terminate.clone(),
			is_queue_empty: self.is_queue_empty.clone(),
			resubmission_request_tx: self.resubmission_request_tx.clone(),
		}
	}
}
pub type ResubmissionQueueTask = Pin<Box<dyn Future<Output = ()> + Send>>;
const CHANNEL_CAPACITY: usize = 100_000;

impl<T: TxTask + 'static> DefaultResubmissionQueue<T> {
	pub fn new() -> (Self, ResubmissionQueueTask) {
		let (resubmission_request_tx, resubmission_request_rx) = mpsc::channel(CHANNEL_CAPACITY);
		let s = Self {
			ready_queue: Default::default(),
			terminate: AtomicBool::new(false).into(),
			forced_terminate: AtomicBool::new(false).into(),
			is_queue_empty: AtomicBool::new(true).into(),
			resubmission_request_tx,
		};
		(s.clone(), s.run(resubmission_request_rx).boxed())
	}

	#[allow(dead_code)]
	async fn run_naive(self, mut resubmission_request_rx: mpsc::Receiver<ResubmittedTxTask<T>>) {
		let mut waiting_queue = FuturesUnordered::<ResubmittedTxTask<T>>::default();
		loop {
			trace!(target: LOG_TARGET, "B RUN ");
			while let Ok(pending) = resubmission_request_rx.try_recv() {
				waiting_queue.push(pending);
			}
			trace!(target: LOG_TARGET, "M RUN ");
			let len = waiting_queue.len();
			trace!(target: LOG_TARGET, "A RUN {}", len);
			if len == 0 {
				self.is_queue_empty.store(true, Ordering::Relaxed);
				if self.terminate.load(Ordering::Relaxed) {
					return;
				}
				tracing::info!(target: LOG_TARGET, "WAIT");
				tokio::time::sleep(Duration::from_millis(100)).await;
			} else {
				self.is_queue_empty.store(false, Ordering::Relaxed);
				let task = {
					trace!(target: LOG_TARGET, "B RUN-QUEUE {}", waiting_queue.len());
					waiting_queue.next().await
				};
				if let Some(t) = task {
					if let Some(t) = t {
						trace!(target: LOG_TARGET, "M B RUN-QUEUE {:?}", t.tx().hash());
						self.ready_queue.write().push(t);
						trace!(target: LOG_TARGET, "M A RUN-QUEUE");
					}
				}
				trace!(target: LOG_TARGET, "A RUN-QUEUE");
			}
		}
	}

	async fn run(self, mut resubmission_request_rx: mpsc::Receiver<ResubmittedTxTask<T>>) {
		let mut waiting_queue = FuturesUnordered::<ResubmittedTxTask<T>>::default();
		loop {
			if self.forced_terminate.load(Ordering::Relaxed) {
				return;
			}
			let len = waiting_queue.len();
			// trace!(target: LOG_TARGET, "B RUN {}", len);
			if len == 0 {
				self.is_queue_empty.store(true, Ordering::Relaxed);
				if self.terminate.load(Ordering::Relaxed) {
					trace!(target: LOG_TARGET, "EXIT RESUBMISSION LOOP");
					return;
				}
				yield_now().await;
			}
			tokio::select! {
				task = waiting_queue.next() => {
					if let Some(t) = task {
						if let Some(t) = t {
							trace!(target: LOG_TARGET, "M RUN-QUEUE {:?}", t.tx().hash());
							self.ready_queue.write().push(t);
							trace!(target: LOG_TARGET, "M A RUN-QUEUE");
						}
					}
				}
				pending = resubmission_request_rx.recv() => {
					if let Some(pending) = pending {
						self.is_queue_empty.store(false, Ordering::Relaxed);
						waiting_queue.push(pending);
					}
				}
			}
			// trace!(target: LOG_TARGET, "A RUN-QUEUE");
		}
	}
}

impl<T: TxTask> DefaultResubmissionQueue<T> {
	async fn wait(t: T) -> Option<T> {
		trace!(target: LOG_TARGET, "B wait {:?}", t.tx().hash());
		tokio::time::sleep(Duration::from_millis(DEFAULT_RESUBMIT_DELAY)).await;
		trace!(target: LOG_TARGET, "A wait {:?}", t.tx().hash());
		t.handle_resubmit_request()
	}

	fn get_lowest_nonce(&self) -> Option<T> {
		let mut queue = self.ready_queue.write();

		if queue.is_empty() {
			return None;
		}

		let mut min_index = 0;
		let mut min_nonce = queue[0].tx().nonce();

		for (i, item) in queue.iter().enumerate().skip(1) {
			let nonce = queue[0].tx().nonce();
			if nonce < min_nonce {
				min_index = i;
				min_nonce = nonce;
			}
		}

		Some(queue.swap_remove(min_index))
	}
}

#[async_trait]
impl<T: TxTask + 'static> ResubmissionQueue<T> for DefaultResubmissionQueue<T> {
	fn forced_terminate(&self) {
		trace!(target: LOG_TARGET, "FORCE TERMINATE ");
		self.forced_terminate.store(true, Ordering::Relaxed);
	}

	fn terminate(&self) {
		trace!(target: LOG_TARGET, "TERMINATE ");
		self.terminate.store(true, Ordering::Relaxed);
	}

	async fn is_empty(&self) -> bool {
		{
			trace!(
				target:LOG_TARGET,
				is_queue_empty = self.is_queue_empty.load(Ordering::Relaxed),
				ready_queue_empty = self.ready_queue.write().is_empty(),
				channel_empty = self.resubmission_request_tx.capacity() == CHANNEL_CAPACITY,
				"is_empty"
			);
		}
		self.is_queue_empty.load(Ordering::Relaxed) &&
			self.ready_queue.write().is_empty() &&
			self.resubmission_request_tx.capacity() == CHANNEL_CAPACITY
	}

	async fn resubmit(&self, task: T, _reason: ResubmitReason) {
		trace!(target: LOG_TARGET, "B PUSH {:?}", task.tx().hash());
		self.resubmission_request_tx
			.send(Box::pin(Self::wait(task)))
			.await
			.expect("send shall not fail");
		trace!(target: LOG_TARGET, "A PUSH");
	}

	//this maybe shall return an error if tx cannot be resubmitted?
	//and runner shall log the this event.
	fn pop(&self) -> Option<T> {
		trace!(target: LOG_TARGET, "B POP");
		// let r = self.ready_queue.write().pop();
		let r = self.get_lowest_nonce();
		trace!(target: LOG_TARGET, "A POP {}", r.is_some());
		r
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::init_logger;
	use tracing::info;

	async fn waitx(i: u32) -> u32 {
		info!(target:LOG_TARGET, "before {}", i);
		tokio::time::sleep(Duration::from_secs(3)).await;
		info!(target:LOG_TARGET, "after {}", i);
		i
	}

	pub type Task = Pin<Box<dyn futures::Future<Output = u32> + Sync + Send>>;

	#[tokio::test]
	async fn resubmit() {
		init_logger();

		let mut q: FuturesUnordered<Task> = Default::default();
		(0..100_000).for_each(|i| q.push(Box::pin(waitx(i))));

		while let Some(i) = q.next().await {
			info!("done: {}", i);
		}
	}
}
