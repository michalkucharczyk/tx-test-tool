#![allow(dead_code)]
#![allow(unused_variables)]

use txtesttool::{
	block_monitor::BlockMonitor,
	cli::{ChainType, Cli, CliCommand},
	execution_log::{journal::Journal, make_stats, STAT_TARGET},
	fake_transaction::FakeTransaction,
	runner::{DefaultTxTask, RunnerFactory},
	scenario::{AccountsDescription, ScenarioExecutor},
	subxt_transaction::{
		self, generate_ecdsa_keypair, generate_sr25519_keypair, EthRuntimeConfig, EthTransaction,
		EthTransactionsSink, SubstrateTransaction, SubstrateTransactionsSink,
	},
	transaction::TransactionRecipe,
};

use clap::Parser;
use parity_scale_codec::Compact;
use std::{fs, fs::File, io::BufReader, time::Duration};
use subxt::{ext::frame_metadata::RuntimeMetadataPrefixed, PolkadotConfig};
use tracing::info;
use txtesttool::init_logger;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	init_logger();

	let cli = Cli::parse();

	match &cli.command {
		CliCommand::Tx {
			chain,
			ws,
			unwatched,
			block_monitor,
			mortal,
			log_file,
			scenario,
			send_threshold,
			remark,
		} => {
			let recipe = remark.map_or_else(TransactionRecipe::transfer, TransactionRecipe::remark);
			let scenario_executor =
				ScenarioExecutor::new(ws, scenario.clone(), recipe, *block_monitor).await;
			match chain {
				ChainType::Fake => {
					let ((stop_runner_tx, runner), queue_task) =
						RunnerFactory::fake_runner(scenario_executor, *send_threshold, *unwatched)
							.await;
				},
				ChainType::Eth => {
					let ((stop_runner_tx, runner), queue_task) =
						RunnerFactory::eth_runner(scenario_executor, *send_threshold, *unwatched)
							.await;
				},
				ChainType::Sub => {
					let ((stop_runner_tx, runner), queue_task) = RunnerFactory::substrate_runner(
						scenario_executor,
						*send_threshold,
						*unwatched,
					)
					.await;
				},
			}
		},
		CliCommand::CheckNonce { chain, ws, account } => {
			match chain {
				ChainType::Fake => {
					panic!("check nonce not supported for fake chain");
				},
				ChainType::Eth => {
					let desc = if let Ok(id) = account.parse::<u32>() {
						AccountsDescription::Derived(id..id + 1)
					} else {
						AccountsDescription::Keyring(account.clone())
					};
					let sink = EthTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						desc,
						generate_ecdsa_keypair,
						None,
					)
					.await;
					let account =
						sink.get_from_account_id(account).ok_or("account shall be correct")?;
					let nonce = sink.check_account_nonce(account).await?;
					info!(target:STAT_TARGET, "{nonce:?}");
				},
				ChainType::Sub => {
					let desc = if let Ok(id) = account.parse::<u32>() {
						AccountsDescription::Derived(id..id + 1)
					} else {
						AccountsDescription::Keyring(account.clone())
					};
					let sink = SubstrateTransactionsSink::new_with_uri_with_accounts_description(
						ws,
						desc,
						generate_sr25519_keypair,
						None,
					)
					.await;
					let account =
						sink.get_from_account_id(account).ok_or("account shall be correct")?;
					let nonce = sink.check_account_nonce(account).await?;
					info!(target:STAT_TARGET, "{nonce:?}");
				},
			};
		},
		CliCommand::Metadata { ws } => {
			// Handle metadata command
			let api = subxt::OnlineClient::<EthRuntimeConfig>::from_insecure_url(ws).await?;
			let runtime_apis = api.runtime_api().at_latest().await?;
			let (_, meta): (Compact<u32>, RuntimeMetadataPrefixed) =
				runtime_apis.call_raw("Metadata_metadata", None).await?;
			println!("{meta:#?}");
		},
		CliCommand::BlockMonitor { chain, ws } => {
			match chain {
				ChainType::Sub => {
					let block_monitor = BlockMonitor::<PolkadotConfig>::new(ws).await;
					async {
						loop {
							tokio::time::sleep(Duration::from_secs(10)).await
						}
					}
					.await;
				},
				ChainType::Eth => {
					let block_monitor = BlockMonitor::<EthRuntimeConfig>::new(ws).await;
					async {
						loop {
							tokio::time::sleep(Duration::from_secs(10)).await
						}
					}
					.await;
				},
				ChainType::Fake => {
					unimplemented!()
				},
			};
		},
		CliCommand::LoadLog { chain, log_file, show_graphs, .. } => match chain {
			ChainType::Sub => {
				let logs = Journal::<DefaultTxTask<SubstrateTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
			ChainType::Eth => {
				let logs = Journal::<DefaultTxTask<EthTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
			ChainType::Fake => {
				let logs = Journal::<DefaultTxTask<FakeTransaction>>::load_logs(log_file);
				make_stats(logs.values().cloned(), *show_graphs);
			},
		},
		CliCommand::GenerateEndowedAccounts {
			chain,
			start_id,
			last_id,
			balance,
			out_file_name,
			chain_spec,
		} => {
			let accounts_description = AccountsDescription::Derived(*start_id..last_id + 1);
			let funded_accounts = match chain {
				ChainType::Sub => {
					let accounts = subxt_transaction::derive_accounts::<PolkadotConfig, _, _>(
						accounts_description.clone(),
						subxt_transaction::SENDER_SEED,
						generate_sr25519_keypair,
					);
					accounts
						.values()
						.map(|keypair| {
							serde_json::json!((
								<PolkadotConfig as subxt::Config>::AccountId::from(
									keypair.0.clone().public_key()
								),
								balance,
							))
						})
						.collect::<Vec<_>>()
				},
				ChainType::Eth => {
					let accounts = subxt_transaction::derive_accounts::<EthRuntimeConfig, _, _>(
						accounts_description.clone(),
						subxt_transaction::SENDER_SEED,
						generate_ecdsa_keypair,
					);
					accounts
						.values()
						.map(|keypair| {
							serde_json::json!((
								"0x".to_string() + &hex::encode(keypair.0.clone().public_key()),
								balance,
							))
						})
						.collect::<Vec<_>>()
				},
				ChainType::Fake => Default::default(),
			};

			if let Some(chain_spec) = chain_spec {
				let file = File::open(chain_spec)?;
				let reader = BufReader::new(file);
				let mut chain_spec: serde_json::Value = serde_json::from_reader(reader)?;

				if let Some(balances) = chain_spec["genesis"]["runtimeGenesis"]["patch"]["balances"]
					["balances"]
					.as_array_mut()
				{
					balances.extend(funded_accounts);
				} else {
					return Err("Balances array not found in provided chain-spec".into());
				}

				fs::write(
					out_file_name,
					serde_json::to_string_pretty(&chain_spec)
						.map_err(|e| format!("to pretty failed: {e}"))?,
				)
				.map_err(|err| err.to_string())?;
			} else {
				let json_object = serde_json::json!({"balances":{"balances":funded_accounts}});
				fs::write(out_file_name, serde_json::to_string_pretty(&json_object)?.as_bytes())?;
			}
		},
	};

	Ok(())
}
