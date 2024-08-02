#![allow(dead_code)]

use tracing::{debug, info, trace};

mod cli;
mod error;
mod execution_log;
mod fake_transaction;
mod fake_transaction_sink;
mod resubmission;
mod runner;
mod subxt_api_connector;
mod subxt_transaction;
mod transaction;
use clap::Parser;

fn init_logger() {
	use std::sync::Once;
	static INIT: Once = Once::new();
	INIT.call_once(|| {
		use tracing_subscriber::{fmt, layer::SubscriberExt, registry, EnvFilter, Layer};

		let filter = EnvFilter::from_default_env();
		let debug_layer = fmt::layer().with_target(true).with_filter(filter);

		let stat_layer =
			fmt::layer().with_target(false).with_level(false).without_time().with_filter(
				tracing_subscriber::filter::filter_fn(|meta| {
					meta.target() == execution_log::STAT_TARGET
				}),
			);

		let subscriber = registry::Registry::default().with(debug_layer).with(stat_layer);

		tracing::subscriber::set_global_default(subscriber)
			.expect("Unable to set a global subscriber");
	});
}

#[tokio::main]
async fn main() {
	init_logger();
	// info!("Hello, world!");
	// debug!(target: "XXX", "Hello, world!");
	// trace!("Hello, world!");

	info!(target: "x", "test");
	debug!(target: "y", "test");
	trace!(target: "z", "test");
	trace!(target: "a", "test");

	let _cli = cli::Cli::parse();

	// match &cli.command {
	// 	Commands::Tx {
	// 		chain,
	// 		ws,
	// 		account,
	// 		nonce,
	// 		unwatched,
	// 		block_monitor,
	// 		mortal,
	// 		log_file,
	// 		tx_command,
	// 	} => {
	// 		match tx_command {
	// 			TxCommands::OneShot { account, nonce } => {
	// 				// Handle one-shot command
	// 			},
	// 			TxCommands::FromSingleAccount { account, from, to, count } => {
	// 				// Handle from-single-account command
	// 			},
	// 			TxCommands::FromManyAccounts { start_id, last_id, count, from, to } => {
	// 				// Handle from-many-accounts command
	// 			},
	// 		}
	// 	},
	// 	Commands::CheckNonce { ws, account } => {
	// 		// Handle check-nonce command
	// 	},
	// 	Commands::Metadata { ws } => {
	// 		// Handle metadata command
	// 	},
	// 	Commands::BlockMonitor { ws } => {
	// 		// Handle block-monitor command
	// 	},
	// 	Commands::LoadLog { log_file, show_graphs, show_errors } => {
	// 		// Handle load-log command
	// 	},
	// }
}
