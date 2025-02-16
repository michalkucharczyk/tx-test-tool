//! This is an experimental library that can be used to send transactions
//! to a substrate network and monitor them in terms of how many have been
//! submitted to the transaction pool, validated, broadcasted, inlcluded in blocks,
//! finalized, invalid or dropped.
//!
//! There is an associated binary that can be used as a CLI, an alternative to using
//! the library.
//!
//! Example:
//!  ```rust,ignore
//!  fn test() {
//!	    // Shared params.
//!     let send_threshold = 20_000;
//!     let ws = "ws://127.0.0.1:9933";
//!     let recipe_future = TransactionRecipe::transfer();
//!     let recipe_ready = recipe_future.clone();
//!     let block_monitor = false;
//!     let unwatched = false;
//!
//!     // Scenarios and sinks.
//!     let scenario_future =
//!       SendingScenario::FromManyAccounts { start_id: 0, last_id: 99, from: Some(100), count: 100
//! };     let scenario_ready =
//!       SendingScenario::FromManyAccounts { start_id: 0, last_id: 99, from: Some(0), count: 100 };
//
//!     let future_scenario_planner =
//!	      ScenarioPlanner::new(ws, scenario_future, recipe_future, block_monitor).await;
//!     let ready_scenario_planner =
//!	      ScenarioPlanner::new(ws, scenario_ready, recipe_ready, block_monitor).await;
//!
//!     let ((future_stop_runner_tx, mut runner_future), future_queue_task) =
//!	      RunnerFactory::substrate_runner(future_scenario_planner, send_threshold,
//! unwatched).await; 	    let ((ready_stop_runner_tx, mut runner_ready), ready_queue_task) =
//!	      RunnerFactory::substrate_runner(ready_scenario_planner, send_threshold, unwatched).await;
//!
//!     let (future_logs, ready_logs) = join(runner_future.run_poc2(),
//! runner_ready.run()).await;     let finalized_future =
//!	      future_logs.values().filter_map(|default_log| default_log.finalized()).count();
//!     let finalized_ready =
//!	      ready_logs.values().filter_map(|default_log| default_log.finalized()).count();
//!     assert_eq!(finalized_future, 100);
//!     assert_eq!(finalized_ready, 100);
//!  }
//!  ```

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
