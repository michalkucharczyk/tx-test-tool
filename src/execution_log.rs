use crate::{error::Error, transaction::TransactionStatus};
use average::{Estimate, Max, Mean, Min, Quantile};
use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
	collections::HashMap,
	sync::Arc,
	time::{Duration, SystemTime},
};
use subxt::config::BlockHash;
use tracing::{info, trace};

pub const STAT_TARGET: &str = "stat";

#[derive(Debug, Serialize, Deserialize)]
pub enum ExecutionEvent<H> {
	Popped(SystemTime),
	Sent(SystemTime),
	Resubmitted(SystemTime),
	SubmitResult(SystemTime, Result<(), String>),
	SubmitAndWatchResult(SystemTime, Result<(), String>),
	TxPoolEvent(SystemTime, TransactionStatus<H>),
}

impl<H: BlockHash + DeserializeOwned + std::fmt::Debug> ExecutionEvent<H> {}

impl<H: BlockHash> ExecutionEvent<H> {
	pub fn resubmitted() -> Self {
		Self::Resubmitted(SystemTime::now())
	}
	pub fn popped() -> Self {
		Self::Popped(SystemTime::now())
	}
	pub fn sent() -> Self {
		Self::Sent(SystemTime::now())
	}
	pub fn submit_and_watch_result(r: Result<(), Error>) -> Self {
		Self::SubmitAndWatchResult(SystemTime::now(), r.map_err(|e| e.to_string()))
	}
	pub fn submit_result(r: Result<(), Error>) -> Self {
		Self::SubmitResult(SystemTime::now(), r.map_err(|e| e.to_string()))
	}
}

impl<H: BlockHash> From<TransactionStatus<H>> for ExecutionEvent<H> {
	fn from(value: TransactionStatus<H>) -> Self {
		Self::TxPoolEvent(SystemTime::now(), value)
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

	fn in_blocks(&self) -> Vec<Self::HashType>;
	fn finalized(&self) -> Option<Self::HashType>;
	fn is_watched(&self) -> bool;

	fn time_to_result(&self) -> Option<Duration>;
	fn time_to_validated(&self) -> Option<Duration>;
	fn time_to_broadcasted(&self) -> Option<Duration>;
	fn time_to_finalized(&self) -> Option<Duration>;
	fn time_to_inblock(&self) -> Option<Duration>;
	fn times_to_inblock(&self) -> Option<Vec<Duration>>;
	fn time_to_dropped(&self) -> Option<Duration>;
	fn time_to_invalid(&self) -> Option<Duration>;
	fn time_to_error(&self) -> Option<Duration>;
	fn time_to_resubmitted(&self) -> Option<Duration>;

	fn get_invalid_reason(&self) -> Option<String>;
	fn get_error_reason(&self) -> Option<String>;
	fn get_dropped_reason(&self) -> Option<String>;
	fn get_resent_count(&self) -> u32;

	fn get_submit_result_error(&self) -> Option<String>;
	fn get_submit_and_watch_result_error(&self) -> Option<String>;
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

impl<H: BlockHash> DefaultExecutionLog<H> {
	fn get_sent_time_stamp(&self) -> Option<SystemTime> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::Sent(i) => Some(*i),
			_ => None,
		})
	}

	fn duration_since_timestamp(
		start: Option<SystemTime>,
		end: Option<SystemTime>,
	) -> Option<Duration> {
		if let (Some(start), Some(end)) = (start, end) {
			Some(end.duration_since(start).expect("time goes forward."))
		} else {
			None
		}
	}
}

impl<H: BlockHash> ExecutionLog for DefaultExecutionLog<H> {
	type HashType = H;

	fn push_event(&self, event: ExecutionEvent<Self::HashType>) {
		trace!(?event, "push_event:");
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

	fn is_watched(&self) -> bool {
		unimplemented!()
	}

	fn in_blocks(&self) -> Vec<Self::HashType> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::TxPoolEvent(_, TransactionStatus::InBlock(h)) => Some(*h),
				_ => None,
			})
			.collect()
	}

	fn finalized(&self) -> Option<Self::HashType> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(_, TransactionStatus::Finalized(h)) => Some(*h),
			_ => None,
		})
	}

	fn time_to_finalized(&self) -> Option<Duration> {
		let fts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Finalized(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), fts)
	}

	fn time_to_validated(&self) -> Option<Duration> {
		let vts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Validated) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), vts)
	}

	fn time_to_broadcasted(&self) -> Option<Duration> {
		let bts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Broadcasted(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), bts)
	}

	fn time_to_inblock(&self) -> Option<Duration> {
		let bts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::InBlock(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), bts)
	}

	fn times_to_inblock(&self) -> Option<Vec<Duration>> {
		unimplemented!()
	}

	fn time_to_dropped(&self) -> Option<Duration> {
		let dts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Dropped(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), dts)
	}

	fn time_to_resubmitted(&self) -> Option<Duration> {
		let dts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::Resubmitted(i) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), dts)
	}

	fn time_to_invalid(&self) -> Option<Duration> {
		let its = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Invalid(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), its)
	}

	fn time_to_error(&self) -> Option<Duration> {
		let ets = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(i, TransactionStatus::Error(_)) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), ets)
	}

	fn time_to_result(&self) -> Option<Duration> {
		let ets = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::SubmitAndWatchResult(i, _) | ExecutionEvent::SubmitResult(i, _) =>
				Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), ets)
	}

	fn get_invalid_reason(&self) -> Option<String> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(_, TransactionStatus::Invalid(reason)) =>
				Some(reason.clone()),
			_ => None,
		})
	}

	fn get_error_reason(&self) -> Option<String> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(_, TransactionStatus::Error(reason)) =>
				Some(reason.clone()),
			_ => None,
		})
	}

	fn get_dropped_reason(&self) -> Option<String> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::TxPoolEvent(_, TransactionStatus::Dropped(reason)) =>
				Some(reason.clone()),
			_ => None,
		})
	}

	fn get_resent_count(&self) -> u32 {
		unimplemented!()
	}

	fn get_submit_result_error(&self) -> Option<String> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::SubmitResult(_, Err(reason)) => Some(reason.clone()),
			_ => None,
		})
	}

	fn get_submit_and_watch_result_error(&self) -> Option<String> {
		self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::SubmitAndWatchResult(_, Err(reason)) => Some(reason.clone()),
			_ => None,
		})
	}
}

pub fn single_stat<'a, E: ExecutionLog + 'a>(
	name: String,
	logs: impl Iterator<Item = &'a Arc<E>>,
	method: fn(&E) -> Option<Duration>,
) {
	let mut v: Vec<f64> = vec![];
	for l in logs {
		let time_to_finalized = method(&*l);
		if let Some(time_to_finalized) = time_to_finalized {
			v.push(time_to_finalized.as_secs_f64());
		}
	}
	let mean: Mean = v.iter().collect();
	let max: Max = v.iter().collect();
	let min: Min = v.iter().collect();
	let mut third_quartile = Quantile::new(0.9);
	v.iter().for_each(|x| third_quartile.add(*x));
	info!(
		target: STAT_TARGET,
		count = mean.len(),
		min = min.min(),
		max = max.max(),
		mean = mean.mean(),
		q90 = third_quartile.quantile(),
		"{name}"
	);

	// use termplot::*;
	// let mut plot = Plot::default();
	// plot.set_domain(Domain(min.min()..max.max()))
	// 	.set_codomain(Domain(0.0..mean.len() as f64))
	// 	.set_title(&name)
	// 	.set_x_label("X axis")
	// 	.set_y_label("Y axis")
	// 	.set_size(Size::new(80, 45))
	// 	.add_plot(Box::new(plot::Histogram::new_with_buckets_count(v, 20)));
	// println!("{plot}");
}

pub fn failure_reason_stats<'a, E: ExecutionLog + 'a>(
	name: String,
	logs: impl Iterator<Item = &'a Arc<E>>,
	method: fn(&E) -> Option<String>,
) {
	let mut map = HashMap::<String, usize>::new();
	for l in logs {
		if let Some(reason) = method(&*l) {
			*map.entry(reason).or_default() += 1;
		}
	}

	info!(?map, "{name} -> {:#?}", map);
}

pub fn make_stats<E: ExecutionLog>(logs: impl IntoIterator<Item = Arc<E>>) {
	let logs = logs.into_iter().collect::<Vec<_>>();
	info!(total_count = logs.iter().count());
	single_stat("Time to dropped".into(), logs.iter(), E::time_to_dropped);
	single_stat("Time to error".into(), logs.iter(), E::time_to_error);
	single_stat("Time to invalid".into(), logs.iter(), E::time_to_invalid);

	single_stat("Time to result".into(), logs.iter(), E::time_to_result);
	single_stat("Time to validated".into(), logs.iter(), E::time_to_validated);
	single_stat("Time to broadcasted".into(), logs.iter(), E::time_to_broadcasted);
	single_stat("Time to in_block".into(), logs.iter(), E::time_to_inblock);
	single_stat("Time to finalization".into(), logs.iter(), E::time_to_finalized);
	single_stat("Time to resubmitted".into(), logs.iter(), E::time_to_resubmitted);

	failure_reason_stats("Dropped".into(), logs.iter(), E::get_dropped_reason);
	failure_reason_stats("Error".into(), logs.iter(), E::get_error_reason);
	failure_reason_stats("Invalid".into(), logs.iter(), E::get_invalid_reason);
	failure_reason_stats("submit errors".into(), logs.iter(), E::get_submit_result_error);
	failure_reason_stats(
		"submit_and_watch errors".into(),
		logs.iter(),
		E::get_submit_and_watch_result_error,
	);

	// let mut v: Vec<f64> = vec![];
	// for l in logs {
	// 	let time_to_finalized = l.time_to_finalized();
	// 	if let Some(time_to_finalized) = time_to_finalized {
	// 		v.push(time_to_finalized.as_secs_f64());
	// 	}
	// 	info!(?time_to_finalized);
	// }
	// use termplot::*;
	// let mut plot = Plot::default();
	// {
	// 	let a: Mean = v.iter().collect();
	// 	info!(?a);
	// }
	// {
	// 	let a: Max = v.iter().collect();
	// 	info!(?a);
	// }
	// {
	// 	let a: Min = v.iter().collect();
	// 	info!(?a);
	// }
	// let mut third_quartile = Quantile::new(0.75);
	// v.iter().for_each(|x| third_quartile.add(*x));
	// info!(q = third_quartile.quantile());
	//
	// plot.set_domain(Domain(0.0..1.0))
	// 	.set_codomain(Domain(0.0..10.0))
	// 	.set_title("Graph title")
	// 	.set_x_label("X axis")
	// 	.set_y_label("Y axis")
	// 	.set_size(Size::new(50, 25))
	// 	.add_plot(Box::new(plot::Histogram::new_with_buckets_count(v, 10)));
	// println!("{plot}");
}