use crate::{
	error::Error,
	runner::{TxTashHash, TxTask},
	transaction::{AccountMetadata, Transaction, TransactionStatus},
};
use average::{Estimate, Max, Mean, Min, Quantile};
use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
	collections::HashMap,
	fmt::Display,
	fs::File,
	io::{Read, Write},
	marker::PhantomData,
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	},
	time::{Duration, SystemTime},
};
use subxt::config::BlockHash;
use tracing::{debug, info, trace};

pub const STAT_TARGET: &str = "stat";
pub const LOG_TARGET: &str = "execution_log";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ExecutionEvent<H> {
	Popped(SystemTime),
	Sent(SystemTime),
	Resubmitted(SystemTime),
	SubmitResult(SystemTime, Result<(), String>),
	SubmitAndWatchResult(SystemTime, Result<(), String>),
	TxPoolEvent(SystemTime, TransactionStatus<H>),
	FinalizedMonitor(SystemTime, H),
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
	pub fn finalized_monitor(block_hash: H) -> Self {
		Self::FinalizedMonitor(SystemTime::now(), block_hash)
	}
}

impl<H: BlockHash> From<TransactionStatus<H>> for ExecutionEvent<H> {
	fn from(value: TransactionStatus<H>) -> Self {
		Self::TxPoolEvent(SystemTime::now(), value)
	}
}

#[derive(Debug, Default)]
pub struct Counters {
	popped: AtomicUsize,
	sent: AtomicUsize,
	resubmitted: AtomicUsize,
	submit_success: AtomicUsize,
	submit_error: AtomicUsize,
	submit_and_watch_success: AtomicUsize,
	submit_and_watch_error: AtomicUsize,
	finalized_monitor: AtomicUsize,

	ts_validated: AtomicUsize,
	ts_broadcasted: AtomicUsize,
	ts_finalized: AtomicUsize,
	ts_dropped: AtomicUsize,
	ts_invalid: AtomicUsize,
	ts_error: AtomicUsize,
}

impl Display for Counters {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// let buffered = self.buffered();
		write!(
			f,
			"p {:7} s:{:7} {:7}/{:7} v:{:7} b{:7} f:{:7} d:{:7} i:{:7}",
			self.popped.load(Ordering::Relaxed),
			self.sent.load(Ordering::Relaxed),
			self.submit_and_watch_success.load(Ordering::Relaxed),
			self.submit_and_watch_error.load(Ordering::Relaxed),
			self.ts_validated.load(Ordering::Relaxed),
			self.ts_broadcasted.load(Ordering::Relaxed),
			self.ts_finalized.load(Ordering::Relaxed),
			self.ts_dropped.load(Ordering::Relaxed),
			self.ts_invalid.load(Ordering::Relaxed),
		)
	}
}

impl Counters {
	fn inc(x: &AtomicUsize) {
		x.fetch_add(1, Ordering::Relaxed);
	}

	pub fn buffered(&self) -> usize {
		self.popped.load(Ordering::Relaxed) -
			(self.submit_and_watch_success.load(Ordering::Relaxed) +
				self.submit_and_watch_error.load(Ordering::Relaxed)) -
			(self.submit_success.load(Ordering::Relaxed) +
				self.submit_error.load(Ordering::Relaxed))
	}

	fn count_event<H: BlockHash>(&self, event: &ExecutionEvent<H>) {
		match event {
			ExecutionEvent::Popped(_) => Self::inc(&self.popped),
			ExecutionEvent::Sent(_) => Self::inc(&self.sent),
			ExecutionEvent::Resubmitted(_) => Self::inc(&self.resubmitted),
			ExecutionEvent::SubmitResult(_, Ok(_)) => Self::inc(&self.submit_success),
			ExecutionEvent::SubmitResult(_, Err(_)) => Self::inc(&self.submit_error),
			ExecutionEvent::SubmitAndWatchResult(_, Ok(_)) =>
				Self::inc(&self.submit_and_watch_success),
			ExecutionEvent::SubmitAndWatchResult(_, Err(_)) =>
				Self::inc(&self.submit_and_watch_error),
			ExecutionEvent::FinalizedMonitor(_, _) => Self::inc(&self.finalized_monitor),
			ExecutionEvent::TxPoolEvent(_, status) => match status {
				TransactionStatus::Validated => Self::inc(&self.ts_validated),
				TransactionStatus::Broadcasted(_) => Self::inc(&self.ts_broadcasted),
				TransactionStatus::Finalized(_) => Self::inc(&self.ts_finalized),
				TransactionStatus::Dropped(_) => Self::inc(&self.ts_dropped),
				TransactionStatus::Invalid(_) => Self::inc(&self.ts_invalid),
				TransactionStatus::Error(_) => Self::inc(&self.ts_error),
				TransactionStatus::NoLongerInBestBlock | TransactionStatus::InBlock(_) => {},
			},
		}
	}
}

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
	fn time_to_finalized_monitor(&self) -> Option<Duration>;

	fn get_invalid_reason(&self) -> Vec<String>;
	fn get_error_reason(&self) -> Vec<String>;
	fn get_dropped_reason(&self) -> Vec<String>;
	fn get_resent_count(&self) -> u32;

	fn get_submit_result_error(&self) -> Vec<String>;
	fn get_submit_and_watch_result_error(&self) -> Vec<String>;

	fn get_inpool_events_string(&self) -> String;
	fn get_sent_time(&self) -> Option<SystemTime>;
}

#[derive(Debug)]
pub struct DefaultExecutionLog<H: BlockHash> {
	events: RwLock<Vec<ExecutionEvent<H>>>,
	account_metadata: AccountMetadata,
	nonce: u128,
	hash: H,
	total_counters: Arc<Counters>,
}

pub type Logs<T> = HashMap<TxTashHash<T>, Arc<DefaultExecutionLog<TxTashHash<T>>>>;

impl<H: BlockHash + Default> Default for DefaultExecutionLog<H> {
	fn default() -> Self {
		Self {
			events: Default::default(),
			nonce: Default::default(),
			account_metadata: Default::default(),
			hash: Default::default(),
			total_counters: Default::default(),
		}
	}
}

impl<H: BlockHash + 'static + Default> DefaultExecutionLog<H> {
	pub fn new_with_events(events: Vec<ExecutionEvent<H>>) -> Self {
		Self { events: events.into(), ..Default::default() }
	}
}

impl<H: BlockHash + 'static> DefaultExecutionLog<H> {
	pub fn new_with_tx(t: &dyn Transaction<HashType = H>, counters: Arc<Counters>) -> Self {
		Self {
			events: Default::default(),
			nonce: t.nonce(),
			account_metadata: t.account_metadata(),
			hash: t.hash(),
			total_counters: counters,
		}
	}

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

impl<H: BlockHash + 'static> ExecutionLog for DefaultExecutionLog<H> {
	type HashType = H;

	fn push_event(&self, event: ExecutionEvent<Self::HashType>) {
		debug!(target:LOG_TARGET, ?event, "B push_event");
		self.total_counters.count_event(&event);
		self.events.write().push(event);
		trace!(target:LOG_TARGET, "A push_event");
	}

	// all methods used for generating stats:
	fn hash(&self) -> Self::HashType {
		self.hash
	}

	fn nonce(&self) -> u128 {
		self.nonce
	}

	fn account_metadata(&self) -> AccountMetadata {
		self.account_metadata.clone()
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

	fn time_to_finalized_monitor(&self) -> Option<Duration> {
		let fmts = self.events.read().iter().find_map(|e| match e {
			ExecutionEvent::FinalizedMonitor(i, _) => Some(*i),
			_ => None,
		});
		Self::duration_since_timestamp(self.get_sent_time_stamp(), fmts)
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

	fn get_invalid_reason(&self) -> Vec<String> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::TxPoolEvent(_, TransactionStatus::Invalid(reason)) =>
					Some(reason.clone()),
				_ => None,
			})
			.collect()
	}

	fn get_error_reason(&self) -> Vec<String> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::TxPoolEvent(_, TransactionStatus::Error(reason)) =>
					Some(reason.clone()),
				_ => None,
			})
			.collect()
	}

	fn get_dropped_reason(&self) -> Vec<String> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::TxPoolEvent(_, TransactionStatus::Dropped(reason)) =>
					Some(reason.clone()),
				_ => None,
			})
			.collect()
	}

	fn get_resent_count(&self) -> u32 {
		unimplemented!()
	}

	fn get_submit_result_error(&self) -> Vec<String> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::SubmitResult(_, Err(reason)) => Some(reason.clone()),
				_ => None,
			})
			.collect()
	}

	fn get_submit_and_watch_result_error(&self) -> Vec<String> {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::SubmitAndWatchResult(_, Err(reason)) => Some(reason.clone()),
				_ => None,
			})
			.collect()
	}

	fn get_inpool_events_string(&self) -> String {
		self.events
			.read()
			.iter()
			.filter_map(|e| match e {
				ExecutionEvent::TxPoolEvent(_, p) => Some(p.get_letter()),
				_ => None,
			})
			.collect()
	}

	fn get_sent_time(&self) -> Option<SystemTime> {
		self.get_sent_time_stamp()
	}
}

pub fn single_stat<'a, E: ExecutionLog + 'a>(
	name: String,
	logs: impl Iterator<Item = &'a Arc<E>>,
	method: fn(&E) -> Option<Duration>,
	show_graph: bool,
) {
	let mut v: Vec<f64> = vec![];
	for l in logs {
		let time_to_event = method(&**l);
		if let Some(time_to_event) = time_to_event {
			v.push(time_to_event.as_secs_f64());
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

	if show_graph {
		use termplot::*;
		let mut plot = Plot::default();
		plot.set_domain(Domain(min.min()..max.max()))
			.set_codomain(Domain(0.0..mean.len() as f64))
			.set_title(&name)
			.set_x_label("X axis")
			.set_y_label("Y axis")
			.set_size(Size::new(80, 45))
			.add_plot(Box::new(plot::Histogram::new_with_buckets_count(v, 20)));
		println!("{plot}");
	}
}

pub fn failure_reason_stats<'a, E: ExecutionLog + 'a>(
	name: String,
	logs: impl Iterator<Item = &'a Arc<E>>,
	method: fn(&E) -> Vec<String>,
) {
	let mut map = HashMap::<String, usize>::new();
	for l in logs {
		for reason in method(&**l) {
			*map.entry(reason).or_default() += 1;
		}
	}

	info!(
		target: STAT_TARGET,
		?map, "{name} -> {:#?}", map);
}

pub fn make_stats<E: ExecutionLog>(logs: impl IntoIterator<Item = Arc<E>>, show_graphs: bool) {
	let logs = logs.into_iter().collect::<Vec<_>>();
	info!(target: STAT_TARGET, total_recorded_count = logs.len());
	single_stat("Time to dropped".into(), logs.iter(), E::time_to_dropped, show_graphs);
	single_stat("Time to error".into(), logs.iter(), E::time_to_error, show_graphs);
	single_stat("Time to invalid".into(), logs.iter(), E::time_to_invalid, show_graphs);

	single_stat("Time to result".into(), logs.iter(), E::time_to_result, show_graphs);
	single_stat("Time to validated".into(), logs.iter(), E::time_to_validated, show_graphs);
	single_stat("Time to broadcasted".into(), logs.iter(), E::time_to_broadcasted, show_graphs);
	single_stat("Time to in_block".into(), logs.iter(), E::time_to_inblock, show_graphs);
	single_stat("Time to finalization".into(), logs.iter(), E::time_to_finalized, show_graphs);
	single_stat(
		"Time to finalization (monitor)".into(),
		logs.iter(),
		E::time_to_finalized_monitor,
		show_graphs,
	);
	single_stat("Time to resubmitted".into(), logs.iter(), E::time_to_resubmitted, show_graphs);

	failure_reason_stats("Dropped".into(), logs.iter(), E::get_dropped_reason);
	failure_reason_stats("Error".into(), logs.iter(), E::get_error_reason);
	failure_reason_stats("Invalid".into(), logs.iter(), E::get_invalid_reason);
	failure_reason_stats("submit errors".into(), logs.iter(), E::get_submit_result_error);
	failure_reason_stats(
		"submit_and_watch errors".into(),
		logs.iter(),
		E::get_submit_and_watch_result_error,
	);

	let mut timeline_map = HashMap::<String, usize>::default();
	let mut logs = logs.into_iter().filter(|e| e.get_sent_time().is_some()).collect::<Vec<_>>();
	logs.sort_by_key(|e| e.get_sent_time().unwrap());
	for e in &logs {
		// info!("{:?}/{:3?} -> {}", e.account_metadata(), e.nonce(), e.get_inpool_events_string());
		*timeline_map.entry(e.get_inpool_events_string()).or_default() += 1;
	}
	timeline_map.iter().for_each(|(l, c)| {
		info!("{:>30} : {:?}", l, c);
	});
	// info!("sorted --------------------");
	// logs.sort_by_key(|e| e.nonce());
	// for e in logs {
	// 	info!("{:?}/{:3?} -> {}", e.account_metadata(), e.nonce(), e.get_inpool_events_string());
	// }
}

pub mod journal {
	use super::*;
	pub struct Journal<T: TxTask> {
		_p: PhantomData<T>,
	}

	//hack
	#[derive(Serialize, Deserialize)]
	struct DefaultExecutionLogSerdeHelper<H> {
		events: Vec<ExecutionEvent<H>>,
		account_metadata: AccountMetadata,
		nonce: u128,
		hash: H,
	}

	impl<H: BlockHash> DefaultExecutionLogSerdeHelper<H> {}

	impl<H: BlockHash> From<DefaultExecutionLogSerdeHelper<H>> for DefaultExecutionLog<H> {
		fn from(value: DefaultExecutionLogSerdeHelper<H>) -> Self {
			DefaultExecutionLog {
				events: value.events.clone().into(),
				account_metadata: value.account_metadata,
				nonce: value.nonce,
				hash: value.hash,
				total_counters: Default::default(),
			}
		}
	}

	impl<H: BlockHash + 'static> DefaultExecutionLog<H> {
		fn get_data(&self) -> DefaultExecutionLogSerdeHelper<H> {
			DefaultExecutionLogSerdeHelper {
				events: self.events.read().clone(),
				account_metadata: self.account_metadata.clone(),
				nonce: self.nonce,
				hash: self.hash,
			}
		}
	}

	impl<T: TxTask> Journal<T>
	where
		TxTashHash<T>: 'static,
	{
		pub fn save_logs(logs: Logs<T>) {
			let data = logs.into_iter().map(|(h, l)| (h, l.get_data())).collect::<HashMap<_, _>>();
			let datetime: chrono::DateTime<chrono::Local> = SystemTime::now().into();
			let filename = format!("out_{}.json", datetime.format("%Y%m%d_%H%M%S"));
			let json = serde_json::to_string(&data).unwrap();
			let mut file = File::create(filename).unwrap();
			file.write_all(json.as_bytes()).unwrap();
		}

		pub fn load_logs(name: &str) -> Logs<T> {
			// Open the file and read its contents
			let mut file = File::open(name).expect("Unable to open file");
			let mut json = String::new();
			file.read_to_string(&mut json).expect("Unable to read file");

			// Deserialize the JSON data into the desired type
			let data: HashMap<TxTashHash<T>, DefaultExecutionLogSerdeHelper<TxTashHash<T>>> =
				serde_json::from_str(&json).expect("Unable to deserialize JSON");

			data.into_iter()
				.map(|l| (l.0, Arc::new(DefaultExecutionLog::from(l.1))))
				.collect()
		}
	}
}
