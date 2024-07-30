use crate::{
	error::Error,
	runner::TxTask,
	transaction::{Transaction, TransactionStatus},
};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, StreamExt};
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
use tracing::info;

#[derive(Debug)]
pub enum ResubmitReason {
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
			}
		}
		return None;
	}
}

#[async_trait]
pub trait ResubmissionQueue<T: TxTask>: Default {
	async fn resubmit(&self, hash: T, reason: ResubmitReason);
	fn pop(&self) -> Option<T>;
	async fn run(&self);
	fn terminate(&self);
	async fn is_empty(&self) -> bool;
}

pub struct DefaultResubmissionQueue<T: TxTask> {
	waiting_queue: Arc<
		tokio::sync::RwLock<
			FuturesUnordered<Pin<Box<dyn futures::Future<Output = Option<T>> + Sync + Send>>>,
		>,
	>,
	ready_queue: Arc<RwLock<Vec<T>>>,
	terminate: Arc<AtomicBool>,
}

impl<T: TxTask> Clone for DefaultResubmissionQueue<T> {
	fn clone(&self) -> Self {
		Self {
			waiting_queue: self.waiting_queue.clone(),
			ready_queue: self.ready_queue.clone(),
			terminate: self.terminate.clone(),
		}
	}
}

impl<T: TxTask> Default for DefaultResubmissionQueue<T> {
	fn default() -> Self {
		Self {
			waiting_queue: Default::default(),
			ready_queue: Default::default(),
			terminate: AtomicBool::new(false).into(),
		}
	}
}

impl<T: TxTask> DefaultResubmissionQueue<T> {
	async fn wait(t: T) -> Option<T> {
		info!(">>>> B wait {:?}", t.tx().hash());
		tokio::time::sleep(Duration::from_millis(3000)).await;
		info!(">>>> A wait {:?}", t.tx().hash());
		t.handle_resubmit_request()
	}
}

#[async_trait]
impl<T: TxTask + 'static> ResubmissionQueue<T> for DefaultResubmissionQueue<T> {
	fn terminate(&self) {
		info!(">>>> TERMINATE ");
		self.terminate.store(true, Ordering::Relaxed);
	}

	async fn is_empty(&self) -> bool {
		self.waiting_queue.write().await.len() == 0 && self.ready_queue.write().is_empty()
	}

	async fn resubmit(&self, task: T, _reason: ResubmitReason) {
		// todo!()
		// task.resubmit()
		// resubmitted.
		info!(">>>> B PUSH {}", self.waiting_queue.write().await.len());
		self.waiting_queue.write().await.push(Box::pin(Self::wait(task)));
		info!(">>>> A PUSH {}", self.waiting_queue.write().await.len());
	}

	//this maybe shall return an error if tx cannot be resubmitted?
	//and runner shall log the this event.
	fn pop(&self) -> Option<T> {
		self.ready_queue.write().pop()
	}

	async fn run(&self) {
		loop {
			info!(">>>> RUN ");
			let mut q = self.waiting_queue.write().await;
			if q.len() == 0 {
				if self.terminate.load(Ordering::Relaxed) {
					return;
				}
				tokio::time::sleep(Duration::from_millis(100)).await;
			} else {
				info!(">>>> RUN {}", q.len());
				while let Some(t) = q.next().await {
					if let Some(t) = t {
						info!(">>>> RUN {:?}", t.tx().hash());
						self.ready_queue.write().push(t)
					}
				}
			}
		}
	}
}
