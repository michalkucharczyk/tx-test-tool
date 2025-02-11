use std::{ops::Range, sync::Arc};

use subxt::config::BlockHash;

use crate::{
	cli::SendingScenario,
	runner::{DefaultTxTask, TxTask},
	transaction::{ResubmitHandler, Transaction, TransactionRecipe, TransactionsSink},
	TransactionBuilder,
};

#[derive(Clone, Debug)]
struct TransactionBuildParams {
	account: String,
	nonce: Option<u128>,
}

#[derive(Debug, Clone)]
pub enum AccountsDescription {
	Keyring(String),
	Derived(Range<u32>),
}

pub struct ScenarioExecutor {
	pub scenario: SendingScenario,
	pub recipe: TransactionRecipe,
	pub block_monitor: bool,
	pub ws: String,
}

impl ScenarioExecutor {
	pub async fn new(
		ws: &str,
		scenario: SendingScenario,
		recipe: TransactionRecipe,
		block_monitor: bool,
	) -> Self {
		Self { scenario, recipe, block_monitor, ws: ws.to_string() }
	}

	pub(crate) async fn generate_transactions<H, T, S, B>(
		&self,
		builder: B,
		sink: S,
		unwatched: bool,
	) -> Vec<DefaultTxTask<T>>
	where
		H: BlockHash + 'static,
		T: Transaction<HashType = H> + ResubmitHandler + Send + 'static,
		S: TransactionsSink<H> + 'static + Clone,
		B: TransactionBuilder<HashType = H, Transaction = T, Sink = S> + Send + Sync + 'static,
	{
		match &self.scenario {
			SendingScenario::OneShot { account, nonce } => {
				vec![
					builder
						.build_transaction(&account, &nonce, &sink, unwatched, &self.recipe)
						.await,
				]
			},
			SendingScenario::FromSingleAccount { account, mut from, count } => {
				let mut tx_build_params = vec![];
				for _ in 0..*count {
					tx_build_params
						.push(TransactionBuildParams { account: account.clone(), nonce: from });
					from = from.map(|n| n + 1);
				}
				self.build_transactions(builder, sink, tx_build_params, unwatched).await
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

				self.build_transactions(builder, sink, tx_build_params, unwatched).await
			},
		}
	}

	async fn build_transactions<H, T, S, B>(
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
	// TODO:
	// - Add tests that check how the nonce is set in the sending scenario is accounted for in
	// transactions building.

	use crate::{
		cli::SendingScenario, fake_transaction_sink::FakeTransactionsSink,
		transaction::TransactionRecipe, FakeTransactionBuilder,
	};

	use super::ScenarioExecutor;

	#[tokio::test]
	async fn test_generate_one_shot() {
		let scenario_executor = ScenarioExecutor::new(
			"",
			SendingScenario::OneShot { account: "0".to_owned(), nonce: Some(50) },
			TransactionRecipe::transfer(),
			false,
		)
		.await;
		// Fake transaction sink builds txs based on nonces computed internally,
		// per given account. Scenario nonces are not taken into considerations.
		let txs = scenario_executor
			.generate_transactions(
				FakeTransactionBuilder::default(),
				FakeTransactionsSink::default(),
				false,
			)
			.await;
		// We're asserting on the number of txs created, not on their nonces.
		assert_eq!(txs.len(), 1);
	}

	#[tokio::test]
	async fn test_generate_from_single_account() {
		let scenario_executor = ScenarioExecutor::new(
			"",
			SendingScenario::FromSingleAccount {
				account: "0".to_string(),
				from: Some(0),
				count: 100,
			},
			TransactionRecipe::transfer(),
			false,
		)
		.await;
		// Fake transaction sink builds txs based on nonces computed internally,
		// per given account. Scenario nonces are not taken into considerations.
		let txs = scenario_executor
			.generate_transactions(
				FakeTransactionBuilder::default(),
				FakeTransactionsSink::default(),
				false,
			)
			.await;
		// We're asserting on the number of txs created, not on their nonces.
		assert_eq!(txs.len(), 100);
	}

	#[tokio::test]
	async fn test_generate_from_many_accounts() {
		let scenario_executor = ScenarioExecutor::new(
			"",
			SendingScenario::FromManyAccounts { start_id: 0, last_id: 2, from: Some(0), count: 30 },
			TransactionRecipe::transfer(),
			false,
		)
		.await;
		// Fake transaction sink builds txs based on nonces computed internally,
		// per given account. Scenario nonces are not taken into considerations.
		let txs = scenario_executor
			.generate_transactions(
				FakeTransactionBuilder::default(),
				FakeTransactionsSink::default(),
				false,
			)
			.await;
		// We're asserting on the number of txs created, not on their nonces.
		assert_eq!(txs.len(), 90);
	}
}
