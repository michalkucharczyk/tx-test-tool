use crate::{
	error::Error,
	transaction::{ResubmitHandler, StreamOf, Transaction, TransactionStatus},
};
use futures::stream::{self};
use futures_util::StreamExt;
use std::{
	any::Any,
	sync::atomic::{AtomicUsize, Ordering},
	time::Duration,
};
use tracing::info;

pub type FakeHash = [u8; 4];

#[derive(Clone)]
pub struct EventDef {
	event: TransactionStatus<FakeHash>,
	delay: u32,
}

impl EventDef {
	pub fn validated(delay: u32) -> Self {
		Self { event: TransactionStatus::Validated, delay }
	}
	pub fn broadcasted(delay: u32, num: u32) -> Self {
		Self {
			//todo
			event: TransactionStatus::Broadcasted(num),
			delay,
		}
	}
	pub fn in_block(h: u32, delay: u32) -> Self {
		Self { event: TransactionStatus::InBlock(h.to_le_bytes()), delay }
	}
	pub fn finalized(h: u32, delay: u32) -> Self {
		Self { event: TransactionStatus::Finalized(h.to_le_bytes()), delay }
	}
	pub fn dropped(delay: u32) -> Self {
		Self { event: TransactionStatus::Dropped("xxx".to_string()), delay }
	}
	pub fn invalid(delay: u32) -> Self {
		Self { event: TransactionStatus::Invalid("xxx".to_string()), delay }
	}
	pub fn error(delay: u32) -> Self {
		Self { event: TransactionStatus::Error("xxx".to_string()), delay }
	}
}

#[derive(Clone)]
pub struct EventsStreamDef(Vec<EventDef>);

impl From<Vec<EventDef>> for EventsStreamDef {
	fn from(value: Vec<EventDef>) -> Self {
		Self(value)
	}
}

pub struct FakeTransaction {
	hash: FakeHash,
	stream_def: Vec<EventsStreamDef>,
	current_stream_def: AtomicUsize,
}

impl Transaction for FakeTransaction {
	type HashType = FakeHash;
	fn hash(&self) -> Self::HashType {
		self.hash
	}
	fn as_any(&self) -> &dyn Any {
		self
	}
}

impl ResubmitHandler for FakeTransaction {
	fn handle_resubmit_request(self) -> Option<Self> {
		self.current_stream_def.fetch_add(1, Ordering::Relaxed);
		if self.current_stream_def.load(Ordering::Relaxed) < self.stream_def.len() {
			Some(self)
		} else {
			None
		}
	}
}

impl FakeTransaction {
	pub fn get_current_stream_def(&self) -> EventsStreamDef {
		self.stream_def[self.current_stream_def.load(Ordering::Relaxed)].clone()
	}

	pub fn new_multiple(hash: u32, stream_def: Vec<EventsStreamDef>) -> Self {
		Self { stream_def, hash: hash.to_le_bytes(), current_stream_def: Default::default() }
	}

	pub fn new(hash: u32, stream_def: EventsStreamDef) -> Self {
		Self {
			stream_def: vec![stream_def],
			hash: hash.to_le_bytes(),
			current_stream_def: Default::default(),
		}
	}

	pub fn new_droppable_2nd_success(hash: u32, delay: u32) -> Self {
		Self::new_multiple(
			hash,
			vec![
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![
					EventDef::broadcasted(10, 3),
					EventDef::validated(10),
					EventDef::in_block(1, 10),
					EventDef::in_block(2, 10),
					EventDef::in_block(3, 10),
					EventDef::finalized(2, 10),
				]),
			],
		)
	}

	pub fn new_droppable(hash: u32, delay: u32) -> Self {
		Self::new(hash, EventsStreamDef(vec![EventDef::dropped(delay)]))
	}

	pub fn new_invalid(hash: u32, delay: u32) -> Self {
		Self::new(hash, EventsStreamDef(vec![EventDef::invalid(delay)]))
	}

	pub fn new_error(hash: u32, delay: u32) -> Self {
		Self::new(hash, EventsStreamDef(vec![EventDef::error(delay)]))
	}

	pub fn new_finalizable_quick(hash: u32) -> Self {
		Self::new(
			hash,
			EventsStreamDef(vec![
				EventDef::broadcasted(10, 3),
				EventDef::validated(10),
				EventDef::in_block(1, 10),
				EventDef::in_block(2, 10),
				EventDef::in_block(3, 10),
				EventDef::finalized(2, 10),
			]),
		)
	}

	pub fn new_finalizable(hash: u32) -> Self {
		Self::new(
			hash,
			EventsStreamDef(vec![
				EventDef::broadcasted(100, 3),
				EventDef::validated(300),
				EventDef::in_block(1, 1000),
				EventDef::in_block(2, 1000),
				EventDef::in_block(3, 1000),
				EventDef::finalized(2, 2000),
			]),
		)
	}

	pub fn events(&self) -> StreamOf<TransactionStatus<FakeHash>> {
		let def = self.get_current_stream_def();
		stream::unfold(def.0.into_iter(), move |mut i| async move {
			if let Some(EventDef { event, delay }) = i.next() {
				tokio::time::sleep(Duration::from_millis(delay.into())).await;
				info!("play: {event:?}");
				Some((event, i))
			} else {
				None
			}
		})
		.boxed()
	}

	pub async fn submit_result(&self) -> Result<FakeHash, Error> {
		let EventDef { event, delay } = self
			.get_current_stream_def()
			.0
			.pop()
			.expect("there shall be at least event. qed.");
		tokio::time::sleep(Duration::from_millis(delay.into())).await;
		info!("submit_result: delayed: {:?}", self.hash);
		match event {
			TransactionStatus::Finalized(_) => Ok(self.hash),
			TransactionStatus::Dropped(message) =>
				Err(Error::Other(format!("submit-error:dropped:{message}").to_string())),
			TransactionStatus::Invalid(message) =>
				Err(Error::Other(format!("submit-error:invalid:{message}").to_string())),
			TransactionStatus::Error(message) =>
				Err(Error::Other(format!("submit-error:error:{message}").to_string())),
			TransactionStatus::Validated |
			TransactionStatus::NoLongerInBestBlock |
			TransactionStatus::Broadcasted(_) |
			TransactionStatus::InBlock(_) => todo!(),
		}
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::init_logger;

	#[tokio::test]
	async fn fake_transaction_events() {
		init_logger();
		let t = FakeTransaction::new(
			1,
			EventsStreamDef(vec![
				EventDef::broadcasted(100, 3),
				EventDef::validated(300),
				EventDef::in_block(1, 1000),
				EventDef::in_block(2, 1000),
				EventDef::in_block(3, 1000),
				EventDef::finalized(2, 2000),
			]),
		);
		let v = t.events().collect::<Vec<_>>().await;
		assert_eq!(
			v,
			vec![
				TransactionStatus::Broadcasted(3),
				TransactionStatus::Validated,
				TransactionStatus::InBlock(1u32.to_le_bytes()),
				TransactionStatus::InBlock(2u32.to_le_bytes()),
				TransactionStatus::InBlock(3u32.to_le_bytes()),
				TransactionStatus::Finalized(2u32.to_le_bytes())
			]
		);
		info!("{v:?}")
	}
}
