use clap::{Parser, Subcommand, ValueEnum};
use std::ops::Range;

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
		#[clap(subcommand)]
		scenario: SendingScenario,
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
		#[clap(long)]
		nonce: Option<u128>,
	},
	/// Send multiple transactions to the node using a single account.
	FromSingleAccount {
		/// Account identifier to be used. It can be keyring account (alice, bob,...) or number of
		/// pre-funded account, index used for derivation.
		#[clap(long, default_value = "alice")]
		account: String,
		/// Starting nonce for 1st transaction in the batch. If not given the current nonce for
		/// the account will be fetched from node for the first transaction in the batch.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch.
		#[clap(long, default_value_t = 1)]
		count: u32,
	},
	/// Send multiple transactions to the node using multiple accounts.
	FromManyAccounts {
		/// First account identifier to be used (index of the pre-funded account used for a
		/// derivation).
		#[clap(long)]
		start_id: u32,
		/// Last account identifier to be used.
		#[clap(long)]
		last_id: u32,
		/// Starting nonce of transactions batch. If not given the current nonce for each account
		/// will be fetched from node.
		#[clap(long)]
		from: Option<u128>,
		/// Number of transaction in the batch per account.
		#[clap(long, default_value_t = 1)]
		count: u32,
	},
}

#[derive(Debug, Clone)]
pub enum AccountsDescription {
	Keyring(String),
	Derived(Range<u32>),
}

impl SendingScenario {
	pub fn get_accounts_description(&self) -> AccountsDescription {
		match self {
			Self::OneShot { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
			Self::FromManyAccounts { start_id, last_id, .. } =>
				AccountsDescription::Derived(*start_id..last_id + 1),
			Self::FromSingleAccount { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
		}
	}
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
