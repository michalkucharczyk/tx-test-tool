use crate::{error::Error, transaction::TransactionStatus};
use parking_lot::RwLock;
use std::time::{Duration, Instant};
use subxt::config::BlockHash;
use tracing::info;

#[derive(Debug)]
pub enum ExecutionEvent<H: BlockHash> {
	Popped(Instant),
	Sent(Instant),
	Resubmitted(Instant),
	SubmitResult(Instant, Result<(), Error>),
	SubmitAndWatchResult(Instant, Result<(), Error>),
	TxPoolEvent(Instant, TransactionStatus<H>),
}

impl<H: BlockHash> ExecutionEvent<H> {
	pub fn popped() -> Self {
		Self::Popped(Instant::now())
	}
	pub fn sent() -> Self {
		Self::Sent(Instant::now())
	}
	pub fn submit_and_watch_result(r: Result<(), Error>) -> Self {
		Self::SubmitAndWatchResult(Instant::now(), r)
	}
	pub fn submit_result(r: Result<(), Error>) -> Self {
		Self::SubmitResult(Instant::now(), r)
	}
}

impl<H: BlockHash> From<TransactionStatus<H>> for ExecutionEvent<H> {
	fn from(value: TransactionStatus<H>) -> Self {
		Self::TxPoolEvent(Instant::now(), value)
	}
}

/// should contain account metadata from sending tool perspecive, e.g. //{}//{idx} used to generate
/// account, or alice/bob maybe call etc...
pub struct AccountMetadata {}

pub trait ExecutionLog: Sync + Send {
	type HashType: BlockHash;

	fn push_event(&self, event: ExecutionEvent<Self::HashType>);

	// all methods used for generating stats:
	fn hash(&self) -> Self::HashType;
	fn nonce(&self) -> u128;
	fn account_metadata(&self) -> AccountMetadata;

	fn in_blocks(&self) -> Option<Vec<Self::HashType>>;
	fn finalized(&self) -> Option<Self::HashType>;
	fn is_watched(&self) -> bool;

	fn time_to_finalized(&self) -> Option<Duration>;
	fn times_to_inblock(&self) -> Option<Vec<Duration>>;
	fn time_to_dropped(&self) -> Option<Duration>;
	fn time_to_invalid(&self) -> Option<Duration>;

	fn get_invalid_reason(&self) -> Option<String>;
	fn get_error_reason(&self) -> Option<String>;
	fn get_dropped_reason(&self) -> Option<String>;
	fn get_resent_count(&self) -> u32;
}

#[derive(Debug)]
pub struct DefaultExecutionLog<H: BlockHash> {
	events: RwLock<Vec<ExecutionEvent<H>>>,
}
impl<H: BlockHash> Default for DefaultExecutionLog<H> {
	fn default() -> Self {
		Self { events: Default::default() }
	}
}

impl<H: BlockHash> ExecutionLog for DefaultExecutionLog<H> {
	type HashType = H;

	fn push_event(&self, event: ExecutionEvent<Self::HashType>) {
		info!(?event, "push_event:");
		self.events.write().push(event);
	}

	// all methods used for generating stats:
	fn hash(&self) -> Self::HashType {
		unimplemented!()
	}
	fn nonce(&self) -> u128 {
		unimplemented!()
	}
	fn account_metadata(&self) -> AccountMetadata {
		unimplemented!()
	}

	fn in_blocks(&self) -> Option<Vec<Self::HashType>> {
		unimplemented!()
	}
	fn finalized(&self) -> Option<Self::HashType> {
		unimplemented!()
	}
	fn is_watched(&self) -> bool {
		unimplemented!()
	}

	fn time_to_finalized(&self) -> Option<Duration> {
		unimplemented!()
	}
	fn times_to_inblock(&self) -> Option<Vec<Duration>> {
		unimplemented!()
	}
	fn time_to_dropped(&self) -> Option<Duration> {
		unimplemented!()
	}
	fn time_to_invalid(&self) -> Option<Duration> {
		unimplemented!()
	}

	fn get_invalid_reason(&self) -> Option<String> {
		unimplemented!()
	}
	fn get_error_reason(&self) -> Option<String> {
		unimplemented!()
	}
	fn get_dropped_reason(&self) -> Option<String> {
		unimplemented!()
	}
	fn get_resent_count(&self) -> u32 {
		unimplemented!()
	}
}
