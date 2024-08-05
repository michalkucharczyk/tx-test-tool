use clap::{Parser, Subcommand, ValueEnum};

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
		/// Send transaction w/o registering event listener.
		#[clap(long)]
		unwatched: bool,
		/// Spawn block monitor for checking if transactions are included in finalized blocks.
		#[clap(long)]
		block_monitor: bool,
		/// Use mortal transactions.
		#[clap(long)]
		mortal: Option<u32>,
		/// Override log file name (out_yyyymmdd_hhmmss.json)
		#[clap(long)]
		log_file: Option<String>,
		#[clap(subcommand)]
		scenario: SendingScenario,
	},
	/// Check nonce for given account.
	CheckNonce {
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
		/// The RPC endpoint of the node to be used.
		#[clap(long, default_value = "ws://127.0.0.1:9933")]
		ws: String,
	},
	/// Load and inspect existing log file.
	LoadLog {
		/// Name of the file to be loaded.
		log_file: String,
		#[clap(long)]
		/// Display some histograms.
		show_graphs: bool,
		/// Display errors.
		#[clap(long)]
		show_errors: bool,
	},
}

#[derive(Subcommand)]
/// Send transactions to the node using different scenarios.
pub enum SendingScenario {
	/// Send single transactions to the node.
	OneShot {
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or number of
		/// pre-funded account, index used for derivation.
		#[clap(long, default_value = "alice")]
		account: String,
		/// Nonce used for the account.
		#[clap(long, default_value = "0")]
		nonce: Option<u128>,
	},
	/// Send multiple transactions to the node using a single account.
	FromSingleAccount {
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or number of
		/// pre-funded account, index used for derivation.
		#[clap(long, default_value = "alice")]
		account: String,
		/// Starting nonce of transactions batch. If not given the current nonce for the account
		/// will be fetched from node.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch.
		#[clap(long, conflicts_with_all(&["from","to"]))]
		count: Option<u32>,
	},
	/// Send multiple transactions to the node using multiple accounts.
	FromManyAccounts {
		/// First account identifier to be used (index of the pre-funded account used for a
		/// derivation).
		#[clap(long)]
		start_id: Option<u32>,
		/// Last account identifier to be used.
		#[clap(long)]
		last_id: Option<u32>,
		/// Starting nonce of transactions batch. If not given the current nonce for the account
		/// will be fetched from node.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch.
		#[clap(long, conflicts_with_all(&["from","to"]))]
		count: Option<u32>,
	},
}

#[derive(ValueEnum, Clone)]
pub enum ChainType {
	/// Substrate compatible chain.
	Sub,
	/// Etheruem compatible chain.
	Eth,
	/// Do not send transactions anywhere, just for dev/testing.
	Fake,
}
