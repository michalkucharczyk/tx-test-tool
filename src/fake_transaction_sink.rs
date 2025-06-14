// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use crate::{
	error::Error,
	fake_transaction::{FakeHash, FakeTransaction},
	helpers::StreamOf,
	transaction::{
		Transaction, TransactionMonitor, TransactionStatus, TransactionStatusIsDone,
		TransactionsSink,
	},
};
use async_trait::async_trait;
use futures_util::StreamExt;
use parking_lot::RwLock;
use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};
use tracing::trace;

#[derive(Default, Clone)]
pub(crate) struct FakeTransactionsSink {
	txs: Arc<RwLock<HashSet<FakeHash>>>,
	#[allow(dead_code)]
	pub(crate) nonces: Arc<RwLock<HashMap<String, u128>>>,
}

#[async_trait]
impl TransactionsSink<FakeHash> for FakeTransactionsSink {
	async fn submit_and_watch(
		&self,
		tx: &dyn Transaction<HashType = FakeHash>,
	) -> Result<StreamOf<TransactionStatus<FakeHash>>, Error> {
		trace!(target: crate::fake_transaction::LOG_TARGET, "submit_and_watch");
		let hash = tx.hash();
		self.txs.write().insert(tx.hash());
		let txs = self.txs.clone();
		let tx = tx.as_any().downcast_ref::<FakeTransaction>().unwrap();
		trace!(target: crate::fake_transaction::LOG_TARGET, hash=?tx.hash(),"submit_and_watch");
		Ok(tx
			.events()
			.map(move |e| {
				if e.is_terminal() {
					txs.write().remove(&hash);
				};
				e
			})
			.boxed())
	}

	async fn submit(&self, tx: &dyn Transaction<HashType = FakeHash>) -> Result<FakeHash, Error> {
		self.txs.write().insert(tx.hash());
		let tx = tx.as_any().downcast_ref::<FakeTransaction>().unwrap();
		let result = tx.submit_result().await;
		self.txs.write().remove(&tx.hash());
		result
	}

	///Current count of transactions being processed by sink
	async fn pending_extrinsics(&self) -> usize {
		self.txs.read().len()
	}

	fn transaction_monitor(&self) -> Option<&dyn TransactionMonitor<FakeHash>> {
		None
	}
}

#[cfg(test)]
mod test {
	use std::time::Duration;

	use super::*;
	use crate::{fake_transaction::EventDef, init_logger};
	use futures::future::join3;
	use tracing::info;

	#[tokio::test]
	async fn fake_sink_submit_and_watch_works() {
		let rpc = FakeTransactionsSink::default();
		let t = FakeTransaction::new_finalizable(1, 0);

		let t: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t);

		let events = rpc.submit_and_watch(&*t).await.unwrap();
		assert_eq!(rpc.pending_extrinsics().await, 1);
		let v = events.collect::<Vec<_>>().await;
		assert_eq!(rpc.pending_extrinsics().await, 0);
		assert_eq!(
			v,
			vec![
				TransactionStatus::Broadcasted,
				TransactionStatus::Validated,
				TransactionStatus::InBlock(1u32.to_le_bytes().into()),
				TransactionStatus::InBlock(2u32.to_le_bytes().into()),
				TransactionStatus::InBlock(3u32.to_le_bytes().into()),
				TransactionStatus::Finalized(2u32.to_le_bytes().into())
			]
		);
	}

	#[tokio::test]
	async fn fake_sink_submit_work_droppable() {
		let rpc = FakeTransactionsSink::default();
		let t = FakeTransaction::new_droppable(1, 0, 10);
		let t: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t);
		let r = rpc.submit(&*t).await.unwrap_err();
		assert_eq!(r.to_string(), Error::Other("submit-error:dropped:xxx".to_string()).to_string());
	}

	#[tokio::test]
	async fn fake_sink_submit_work_invalid() {
		let rpc = FakeTransactionsSink::default();
		let t = FakeTransaction::new_invalid(1, 0, 10);
		let t: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t);
		let r = rpc.submit(&*t).await.unwrap_err();
		assert_eq!(r.to_string(), Error::Other("submit-error:invalid:xxx".to_string()).to_string());
	}

	#[tokio::test]
	async fn fake_sink_submit_work_error() {
		let rpc = FakeTransactionsSink::default();
		let t = FakeTransaction::new_error(1, 0, 10);
		let t: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t);
		let r = rpc.submit(&*t).await.unwrap_err();
		assert_eq!(r.to_string(), Error::Other("submit-error:error:xxx".to_string()).to_string());
	}

	#[tokio::test]
	async fn fake_sink_submit_work_valid() {
		let rpc = FakeTransactionsSink::default();
		let t = FakeTransaction::new_finalizable_quick(111, 0);
		let t: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t);
		let r = rpc.submit(&*t).await.unwrap();
		assert_eq!(r, 111u32.to_le_bytes().into());
	}

	#[tokio::test]
	async fn fake_sink_submit_work_valid_2() {
		init_logger();
		info!("start");
		let rpc = Arc::from(FakeTransactionsSink::default());
		let t1 = FakeTransaction::new(111, 0, vec![EventDef::finalized(11, 250)].into());
		let t2 = FakeTransaction::new(222, 0, vec![EventDef::finalized(12, 250)].into());
		let t1: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t1);
		let t2: Box<dyn Transaction<HashType = FakeHash>> = Box::from(t2);

		let f = || async {
			tokio::time::sleep(Duration::from_millis(200)).await;
			rpc.pending_extrinsics().await
		};

		let result = join3(rpc.submit(&*t1), rpc.submit(&*t2), f()).await;
		let r1 = result.0;
		let r2 = result.1;
		assert_eq!(result.2, 2);
		assert_eq!(rpc.pending_extrinsics().await, 0);
		assert_eq!(r1.unwrap(), 111u32.to_le_bytes().into());
		assert_eq!(r2.unwrap(), 222u32.to_le_bytes().into());
	}

	//todo: submit_and_watch can return error
}
