// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

//! Example: Send `TestPallet::kill_dev_entry` transactions using a custom payload builder.
//!
//! This example demonstrates how to use `with_tx_payload_builder_sub` to build a custom spammer
//! tool using specific transactions.
//!
//! Usage:
//! ```bash
//! cargo run --example kill_dev_entry -- --start-id 0 --last-id 99
//! ```

use clap::Parser;
use substrate_txtesttool::scenario::{ChainType, ScenarioBuilder};
use subxt::dynamic::Value;

#[derive(Parser)]
#[clap(name = "kill_dev_entry")]
struct Cli {
	/// Start account ID (inclusive)
	#[clap(long)]
	start_id: u32,

	/// Last account ID (inclusive)
	#[clap(long)]
	last_id: u32,

	/// The RPC endpoint of the node to be used.
	#[clap(long, default_value = "ws://127.0.0.1:9933")]
	ws: String,

	/// Send transactions threshold
	#[clap(long, default_value_t = 10000)]
	send_threshold: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	substrate_txtesttool::init_logger();

	let cli = Cli::parse();

	let scenario_builder = ScenarioBuilder::new()
		.with_rpc_uri(cli.ws)
		.with_watched_txs(true)
		.with_chain_type(ChainType::Sub)
		.with_send_threshold(cli.send_threshold as usize)
		.with_start_id(cli.start_id)
		.with_last_id(cli.last_id)
		.with_txs_count(1)
		.with_legacy_backend(true)
		.with_installed_ctrlc_stop_hook(true)
		.with_tx_payload_builder_sub(|ctx| {
			let x = ctx.account.parse::<u32>().unwrap();
			const BATCH_SIZE: u32 = 5;
			let start = Value::u128((BATCH_SIZE * x) as u128);
			let count = Value::u128(BATCH_SIZE.into());
			subxt::dynamic::tx("TestPallet", "kill_dev_entry", vec![start, count])
		});

	let scenario_executor = scenario_builder.build().await;
	let _ = scenario_executor.execute().await;

	Ok(())
}
