// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

//! The Transaction Test Tool is a library allowing to send transactions to a substrate network,
//! monitor their status within the transaction pool. The main purpose of the library is to put a
//! network under different scenarios and ensure transaction pool behaves as expected.
//!
//! Additionally, there is a companion command-line interface (CLI) that offers an alternative
//! means of utilizing the library's capabilities.
//!
//! Example:
//!
//! ```rust,ignore
//!     // Shared Params
//! 	let send_threshold = 20_000;
//! 	let ws = "ws://127.0.0.1:9933";
//! 	let block_monitor = false;
//! 	let watched_txs = true;
//!
//!     // Setup for scenario executor
//! 	let scenario_executor = ScenarioBuilder::new()
//! 		.with_rpc_uri(ws.to_string())
//! 		.with_chain_type(ChainType::Sub)
//! 		.with_block_monitoring(block_monitor)
//! 		.with_start_id("0".to_string())
//! 		.with_last_id(99)
//! 		.with_nonce_from(Some(0))
//! 		.with_txs_count(100)
//! 		.with_watched_txs(watched_txs)
//! 		.with_send_threshold(send_threshold)
//! 		.build()
//! 		.await;
//!
//! 	let logs = scenario_executor.execute().await;
//! ```

pub mod block_monitor;
pub mod cli;
pub mod error;
pub mod execution_log;
pub mod fake_transaction;
pub mod fake_transaction_sink;
pub mod helpers;
pub mod resubmission;
pub mod runner;
pub mod scenario;
pub mod subxt_api_connector;
pub mod subxt_transaction;
pub mod transaction;

/// Initialize the logger for various binaries (e.g. ttxt or test binaries).
pub fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		use tracing::Metadata;
		use tracing_subscriber::{
			fmt,
			layer::{Context, Filter, SubscriberExt},
			registry, EnvFilter, Layer,
		};

		let filter = EnvFilter::from_default_env();

		struct F {
			env_filter: EnvFilter,
		}

		impl Default for F {
			fn default() -> Self {
				Self { env_filter: EnvFilter::from_default_env() }
			}
		}
		impl<S> Filter<S> for F {
			fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
				!self.env_filter.enabled(meta, cx.clone()) &&
					meta.target() == execution_log::STAT_TARGET
			}
		}

		let debug_layer = fmt::layer().with_target(true).with_filter(filter);
		let stat_layer = fmt::layer()
			.with_target(false)
			.with_level(false)
			.without_time()
			.with_filter(F::default());

		let subscriber = registry::Registry::default().with(debug_layer).with(stat_layer);

		tracing::subscriber::set_global_default(subscriber)
			.expect("Unable to set a global subscriber");
	});
}
