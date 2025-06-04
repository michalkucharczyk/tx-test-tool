// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use crate::{
	block_monitor::BlockMonitorDisplayOptions,
	scenario::{ChainType, ScenarioType},
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[clap(name = "txtt")]
pub struct Cli {
	#[clap(subcommand)]
	pub command: CliCommand,
}

#[derive(Subcommand)]
pub enum CliCommand {
	Tx {
		/// The type of chain to be used.
		#[clap(long, default_value = "sub")]
		chain: ChainType,
		/// The RPC endpoint of the node to be used.
		#[clap(long, default_value = "ws://127.0.0.1:9933")]
		ws: String,
		/// Send transaction with event listener (submit_and_watch).
		#[clap(long)]
		unwatched: bool,
		/// Spawn block monitor for checking if transactions are included in finalized blocks.
		#[clap(long)]
		block_monitor: bool,
		/// Use mortal transactions.
		#[clap(long)]
		mortal: Option<u32>,
		/// Send transactions threshold, sends the batch when number of pedning extrinsics drops
		/// below this number.
		#[clap(long, default_value_t = 10000)]
		send_threshold: u32,
		/// Override log file name (out_yyyymmdd_hhmmss.json)
		#[clap(long)]
		log_file: Option<String>,
		/// Use remark command with given size in kbytes. If not given transfer transaction will be
		/// sent.
		#[clap(long)]
		remark: Option<u32>,
		/// Transaction tip (allows to control prio)
		#[clap(long, default_value_t = 0)]
		tip: u128,
		/// Accounts range used for building/seding transactions.
		#[clap(subcommand)]
		scenario: ScenarioType,
		/// Use legacy backend
		#[clap(long, default_value_t = false)]
		use_legacy_backend: bool,
	},
	/// Check nonce for given account.
	CheckNonce {
		/// The type of chain to be used.
		#[clap(long, default_value = "sub")]
		chain: ChainType,
		/// The RPC endpoint of the node to be used.
		#[clap(long, default_value = "ws://127.0.0.1:9933")]
		ws: String,
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or index of
		/// pre-funded account index used for derivation.
		#[clap(long)]
		account: String,
	},
	/// Download and display the metadata.
	Metadata {
		/// The RPC endpoint of the node to be used.
		#[clap(long, default_value = "ws://127.0.0.1:9933")]
		ws: String,
	},
	/// Execute the stand alone block monitor and print some transactions stats.
	BlockMonitor {
		/// The type of chain to be used.
		#[clap(long, default_value = "sub")]
		chain: ChainType,
		/// The RPC endpoint of the node to be used.
		#[clap(long, default_value = "ws://127.0.0.1:9933")]
		ws: String,
		#[clap(long, default_value = "all")]
		display: BlockMonitorDisplayOptions,
	},
	/// Load and inspect existing log file.
	LoadLog {
		/// The type of chain used to store the log file..
		#[clap(long, default_value = "sub")]
		chain: ChainType,
		/// Name of the file to be loaded.
		log_file: String,
		#[clap(long)]
		/// Display some histograms.
		show_graphs: bool,
		/// Display errors.
		#[clap(long)]
		show_errors: bool,
		/// Convert loaded log file into CSV file. If specified, CSV file will be written into
		/// given location.
		#[clap(long)]
		out_csv_filename: Option<String>,
	},
	/// Generate a list of endowed accounts.
	GenerateEndowedAccounts {
		/// The type of chain used to store the log file..
		#[clap(long, default_value = "sub")]
		chain: ChainType,
		/// First account identifier to be generated (index of the account used for a
		/// derivation).
		#[clap(long)]
		start_id: u32,
		/// Last account identifier to be generated.
		#[clap(long)]
		last_id: u32,
		/// Initial balance
		#[clap(long, default_value = "100000000000000")]
		balance: u128,
		/// File where patch with funded accounts json will be stored. Or if the input chain-spec
		/// was provided, the location of chain spec with endowed accounts injected.
		#[clap(long)]
		out_file_name: String,
		/// The plain chain spec file that will be used to inject endowed accounts into.
		#[clap(long)]
		chain_spec: Option<String>,
	},
}
