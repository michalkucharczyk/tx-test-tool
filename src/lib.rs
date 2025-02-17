//! # Transaction Test Tool
//!
//! This is an experimental library that can be used to send transactions
//! to a substrate network and monitor them in terms of how many have been
//! submitted to the transaction pool, validated, broadcasted, inlcluded in blocks,
//! finalized, invalid or dropped. The main purpose of the library is to put under a network under
//! different scenarios and ensure transaction pool behaves as expected.
//!
//! There is an associated binary that can be used as a CLI, an alternative to using
//! the library.

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
