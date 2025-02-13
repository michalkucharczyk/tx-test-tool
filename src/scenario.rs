use std::{ops::Range, sync::Arc};

use clap::Subcommand;
use subxt::config::BlockHash;

use crate::{
	runner::{DefaultTxTask, TxTask},
	transaction::{ResubmitHandler, Transaction, TransactionRecipe, TransactionsSink},
	TransactionBuilder,
};

#[derive(Clone, Debug)]
/// Holds information relevant for transaction generation.
pub(crate) struct TransactionBuildParams {
	pub account: String,
	pub nonce: Option<u128>,
}

#[derive(Debug, Clone)]
/// Describes the account types that will participate
/// in a [`SendingScenario`].
pub enum AccountsDescription {
	Keyring(String),
	Derived(Range<u32>),
}

#[derive(Subcommand, Clone)]
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

impl From<&SendingScenario> for AccountsDescription {
	fn from(value: &SendingScenario) -> Self {
		match value {
			SendingScenario::OneShot { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
			SendingScenario::FromManyAccounts { start_id, last_id, .. } =>
				AccountsDescription::Derived(*start_id..last_id + 1),
			SendingScenario::FromSingleAccount { account, .. } =>
				if let Ok(id) = account.parse::<u32>() {
					AccountsDescription::Derived(id..id + 1)
				} else {
					AccountsDescription::Keyring(account.clone())
				},
		}
	}
}

impl SendingScenario {
	pub(crate) fn as_tx_build_params(&self) -> Vec<TransactionBuildParams> {
		match self {
			SendingScenario::OneShot { account, nonce } => {
				vec![TransactionBuildParams { account: account.clone(), nonce: *nonce }]
			},
			SendingScenario::FromSingleAccount { account, mut from, count } => {
				let mut tx_build_params = vec![];
				for _ in 0..*count {
					tx_build_params
						.push(TransactionBuildParams { account: account.clone(), nonce: from });
					from = from.map(|n| n + 1);
				}
				tx_build_params
			},
			SendingScenario::FromManyAccounts { start_id, last_id, from, count } => {
				let mut tx_build_params = vec![];
				for account in *start_id..=*last_id {
					let mut nonce = from.clone();
					for _ in 0..*count {
						tx_build_params
							.push(TransactionBuildParams { account: account.to_string(), nonce });
						nonce = nonce.map(|n| n + 1);
					}
				}
				tx_build_params
			},
		}
	}
}

/// Plans the execution of generated transactions, based on a [`SendingScenario`]
/// and transaction recipe.
pub struct ScenarioPlanner {
	pub scenario: SendingScenario,
	pub recipe: TransactionRecipe,
	pub has_block_monitor: bool,
	pub ws: String,
}

impl ScenarioPlanner {
	pub async fn new(
		ws: &str,
		scenario: SendingScenario,
		recipe: TransactionRecipe,
		block_monitor: bool,
	) -> Self {
		Self { scenario, recipe, has_block_monitor: block_monitor, ws: ws.to_string() }
	}

	/// Returns a set of tasks that handle transaction execution.
	pub(crate) async fn build_transactions<H, T, S, B>(
		&self,
		builder: B,
		sink: S,
		tx_build_params: Vec<TransactionBuildParams>,
		unwatched: bool,
	) -> Vec<DefaultTxTask<T>>
	where
		H: BlockHash + 'static,
		T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
		S: TransactionsSink<H> + 'static + Clone,
		B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
	{
		let n = tx_build_params.len();
		let t = std::cmp::min(
			n,
			std::thread::available_parallelism().unwrap_or(1usize.try_into().unwrap()).get(),
		);
		let tx_build_params = Arc::<Vec<TransactionBuildParams>>::from(tx_build_params);
		let builder = Arc::new(builder);
		let mut threads = Vec::new();

		(0..t).for_each(|thread_idx| {
			let chunk = ((thread_idx * n) / t)..(((thread_idx + 1) * n) / t);
			let tx_build_params = tx_build_params.clone();
			let builder = builder.clone();
			let sink = sink.clone();
			let recipe = self.recipe.clone();
			threads.push(tokio::task::spawn(async move {
				let mut txs = vec![];
				for i in chunk {
					let build_params = tx_build_params[i].clone();
					txs.push(
						builder
							.build_transaction(
								&build_params.account,
								&build_params.nonce,
								&sink,
								unwatched,
								&recipe,
							)
							.await,
					);
				}
				txs
			}));
		});

		let mut results = vec![];
		for handle in threads {
			let result = handle.await.unwrap();
			results.push(result);
		}
		let mut txs: Vec<_> = results.into_iter().flatten().collect();
		txs.sort_by_key(|k| k.tx().nonce());
		txs
	}
}

#[cfg(test)]
mod tests {
	use super::SendingScenario;
	#[test]
	fn sending_scenario_as_tx_build_params() {
		let scenario = SendingScenario::OneShot { account: "0".to_string(), nonce: Some(0) };
		let tx_build_params = scenario.as_tx_build_params();
		assert_eq!(tx_build_params.len(), 1);
		assert_eq!(tx_build_params[0].account, "0".to_string());
		assert_eq!(tx_build_params[0].nonce, Some(0));

		let scenario = SendingScenario::FromSingleAccount {
			account: "0".to_string(),
			from: Some(0),
			count: 10,
		};
		let tx_build_params = scenario.as_tx_build_params();
		assert_eq!(tx_build_params.len(), 10);
		for (i, params) in tx_build_params.iter().enumerate() {
			assert_eq!(params.account, "0".to_string());
			assert_eq!(params.nonce, Some(i as u128));
		}

		let scenario = SendingScenario::FromManyAccounts {
			start_id: 0,
			last_id: 10,
			from: Some(0),
			count: 100,
		};
		let tx_build_params = scenario.as_tx_build_params();
		assert_eq!(tx_build_params.len(), 1100);
		for (nonce, params) in tx_build_params.iter().enumerate() {
			assert_eq!(params.account, (nonce / 100).to_string());
			assert_eq!(params.nonce, Some(nonce as u128 % 100));
		}
	}
}
