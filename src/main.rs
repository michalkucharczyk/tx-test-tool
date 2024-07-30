//todo:
#![allow(dead_code)]
//todo:
#![allow(unused_imports)]
use async_trait::async_trait;
use futures::stream::{self};
use futures_util::{stream::FuturesUnordered, StreamExt};
use parking_lot::RwLock;
use rand::Rng;
use std::{
	any::Any,
	collections::HashSet,
	pin::Pin,
	sync::Arc,
	time::{Duration, Instant},
};
use subxt::{self, tx::TxStatus, OnlineClient};
use subxt_core::config::BlockHash;
use tokio::select;
use tracing::info;
use tracing_subscriber;

mod error;
mod execution_log;
mod fake_transaction;
mod fake_transaction_sink;
mod resubmission;
mod runner;
mod subxt_api_connector;
mod subxt_transaction;
mod transaction;

fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		// tracing_subscriber::fmt().with_max_level(Level::INFO).init();
		let timer = tracing_subscriber::fmt::time::OffsetTime::new(
			time::macros::offset!(+2),
			time::macros::format_description!(
				"[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
			),
		);
		tracing_subscriber::fmt().with_timer(timer).init();
	});
}

#[tokio::main]
async fn main() {
	init_logger();
	info!("Hello, world!");
}
