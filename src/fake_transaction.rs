use crate::{
	error::Error,
	helpers::StreamOf,
	transaction::{AccountMetadata, ResubmitHandler, Transaction, TransactionStatus},
};
use futures::stream::{self};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
	any::Any,
	fmt,
	str::FromStr,
	sync::atomic::{AtomicUsize, Ordering},
	time::Duration,
};
use subxt::ext::codec::{Decode, Encode};
use tokio::task::yield_now;
use tracing::trace;

pub(crate) const LOG_TARGET: &str = "fake_tx";

#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct FakeHash([u8; 4]);

impl AsRef<[u8]> for FakeHash {
	fn as_ref(&self) -> &[u8] {
		&self.0
	}
}

impl From<[u8; 4]> for FakeHash {
	fn from(value: [u8; 4]) -> Self {
		Self(value)
	}
}

impl fmt::Debug for FakeHash {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

impl fmt::Display for FakeHash {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", hex::encode(self.0))
	}
}

impl FromStr for FakeHash {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let mut bytes = [0; 4];
		hex::decode_to_slice(s, &mut bytes).map_err(|_| "hex::decode failed".to_string())?;
		Ok(FakeHash(bytes))
	}
}

impl From<FakeHash> for String {
	fn from(hash: FakeHash) -> Self {
		hex::encode(hash.0)
	}
}

impl TryFrom<String> for FakeHash {
	type Error = String;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		FakeHash::from_str(&value)
	}
}

#[derive(Clone)]
pub struct EventDef {
	event: TransactionStatus<FakeHash>,
	delay: u32,
}

impl EventDef {
	pub fn validated(delay: u32) -> Self {
		Self { event: TransactionStatus::Validated, delay }
	}
	pub fn broadcasted(delay: u32) -> Self {
		Self {
			//todo
			event: TransactionStatus::Broadcasted,
			delay,
		}
	}
	pub fn in_block(h: u32, delay: u32) -> Self {
		Self { event: TransactionStatus::InBlock(h.to_le_bytes().into()), delay }
	}
	pub fn finalized(h: u32, delay: u32) -> Self {
		Self { event: TransactionStatus::Finalized(h.to_le_bytes().into()), delay }
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
pub(crate) struct EventsStreamDef(pub Vec<EventDef>);

impl From<Vec<EventDef>> for EventsStreamDef {
	fn from(value: Vec<EventDef>) -> Self {
		Self(value)
	}
}

pub(crate) struct FakeTransaction {
	hash: FakeHash,
	stream_def: Vec<EventsStreamDef>,
	current_stream_def: AtomicUsize,
	nonce: u128,
	account_metadata: AccountMetadata,
}

impl Transaction for FakeTransaction {
	type HashType = FakeHash;
	fn hash(&self) -> Self::HashType {
		self.hash
	}
	fn as_any(&self) -> &dyn Any {
		self
	}
	fn nonce(&self) -> u128 {
		self.nonce
	}
	fn account_metadata(&self) -> AccountMetadata {
		self.account_metadata.clone()
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

#[allow(dead_code)]
impl FakeTransaction {
	pub(crate) fn get_current_stream_def(&self) -> EventsStreamDef {
		self.stream_def[self.current_stream_def.load(Ordering::Relaxed)].clone()
	}

	pub(crate) fn new_multiple(index: u32, nonce: u128, stream_def: Vec<EventsStreamDef>) -> Self {
		Self {
			stream_def,
			hash: index.to_le_bytes().into(),
			current_stream_def: Default::default(),
			nonce,
			account_metadata: AccountMetadata::Derived(index),
		}
	}

	pub(crate) fn new_with_keyring(
		account: String,
		nonce: u128,
		stream_def: Vec<EventsStreamDef>,
	) -> Self {
		let acc_as_bytes = account.as_bytes();
		Self {
			stream_def,
			hash: [acc_as_bytes[0], acc_as_bytes[1], acc_as_bytes[2], acc_as_bytes[3]].into(),
			current_stream_def: Default::default(),
			nonce,
			account_metadata: AccountMetadata::KeyRing(account),
		}
	}

	pub(crate) fn new(index: u32, nonce: u128, stream_def: EventsStreamDef) -> Self {
		Self {
			stream_def: vec![stream_def],
			hash: index.to_le_bytes().into(),
			current_stream_def: Default::default(),
			nonce,
			account_metadata: AccountMetadata::Derived(index),
		}
	}

	pub(crate) fn new_inblock_then_droppable_2nd_success(
		hash: u32,
		nonce: u128,
		delay: u32,
	) -> Self {
		Self::new_multiple(
			hash,
			nonce,
			vec![
				EventsStreamDef(vec![
					EventDef::broadcasted(delay),
					EventDef::validated(delay),
					EventDef::in_block(1, delay),
					EventDef::in_block(2, delay),
					EventDef::in_block(3, delay),
					EventDef::dropped(delay),
				]),
				EventsStreamDef(vec![
					EventDef::broadcasted(delay),
					EventDef::validated(delay),
					EventDef::in_block(1, delay),
					EventDef::in_block(2, delay),
					EventDef::in_block(3, delay),
					EventDef::finalized(2, delay),
				]),
			],
		)
	}

	pub(crate) fn new_droppable_2nd_success(hash: u32, nonce: u128, delay: u32) -> Self {
		Self::new_multiple(
			hash,
			nonce,
			vec![
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![
					EventDef::broadcasted(delay),
					EventDef::validated(delay),
					EventDef::in_block(1, delay),
					EventDef::in_block(2, delay),
					EventDef::in_block(3, delay),
					EventDef::finalized(2, delay),
				]),
			],
		)
	}

	pub(crate) fn new_droppable_loop(hash: u32, nonce: u128, delay: u32) -> Self {
		Self::new_multiple(
			hash,
			nonce,
			vec![
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![EventDef::dropped(delay)]),
				EventsStreamDef(vec![EventDef::dropped(delay)]),
			],
		)
	}

	pub(crate) fn new_droppable(hash: u32, nonce: u128, delay: u32) -> Self {
		Self::new(hash, nonce, EventsStreamDef(vec![EventDef::dropped(delay)]))
	}

	pub(crate) fn new_invalid(hash: u32, nonce: u128, delay: u32) -> Self {
		Self::new(hash, nonce, EventsStreamDef(vec![EventDef::invalid(delay)]))
	}

	pub(crate) fn new_error(hash: u32, nonce: u128, delay: u32) -> Self {
		Self::new(hash, nonce, EventsStreamDef(vec![EventDef::error(delay)]))
	}

	pub(crate) fn new_finalizable_quick(hash: u32, nonce: u128) -> Self {
		let delay = 0;
		Self::new(
			hash,
			nonce,
			EventsStreamDef(vec![
				EventDef::broadcasted(delay),
				EventDef::validated(delay),
				EventDef::in_block(1, delay),
				EventDef::in_block(2, delay),
				EventDef::in_block(3, delay),
				EventDef::finalized(2, delay),
			]),
		)
	}

	pub(crate) fn new_finalizable(hash: u32, nonce: u128) -> Self {
		Self::new(
			hash,
			nonce,
			EventsStreamDef(vec![
				EventDef::broadcasted(100),
				EventDef::validated(300),
				EventDef::in_block(1, 1000),
				EventDef::in_block(2, 1000),
				EventDef::in_block(3, 1000),
				EventDef::finalized(2, 2000),
			]),
		)
	}

	pub(crate) fn events(&self) -> StreamOf<TransactionStatus<FakeHash>> {
		let def = self.get_current_stream_def();
		stream::unfold(def.0.into_iter(), move |mut i| async {
			yield_now().await;
			if let Some(EventDef { event, delay }) = i.next() {
				if delay > 0 {
					tokio::time::sleep(Duration::from_millis(delay.into())).await;
				}
				trace!(target:LOG_TARGET, ?event, ?delay, "play");
				Some((event, i))
			} else {
				None
			}
		})
		.boxed()
	}

	pub(crate) async fn submit_result(&self) -> Result<FakeHash, Error> {
		let EventDef { event, delay } = self
			.get_current_stream_def()
			.0
			.pop()
			.expect("there shall be at least event. qed.");
		if delay > 0 {
			tokio::time::sleep(Duration::from_millis(delay.into())).await;
		}
		trace!(target:LOG_TARGET, "submit_result: delayed: {:?}", self.hash);
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
			TransactionStatus::Broadcasted |
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
			0,
			EventsStreamDef(vec![
				EventDef::broadcasted(100),
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
				TransactionStatus::Broadcasted,
				TransactionStatus::Validated,
				TransactionStatus::InBlock(1u32.to_le_bytes().into()),
				TransactionStatus::InBlock(2u32.to_le_bytes().into()),
				TransactionStatus::InBlock(3u32.to_le_bytes().into()),
				TransactionStatus::Finalized(2u32.to_le_bytes().into())
			]
		);
		tracing::info!("{v:?}")
	}
}
