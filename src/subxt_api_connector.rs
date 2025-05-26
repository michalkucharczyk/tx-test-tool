// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use std::{error::Error, sync::Arc, time::Duration};
use subxt::OnlineClient;
use tracing::info;

use crate::helpers;

const MAX_ATTEMPTS: usize = 10;
/// Delay period between failed connection attempts.
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Connect to a RPC node.
pub(crate) async fn connect<C: subxt::Config>(
	url: &str,
	use_legacy_backend: bool,
) -> Result<OnlineClient<C>, Box<dyn Error>> {
	for i in 0..MAX_ATTEMPTS {
		info!("Attempt #{}: Connecting to {}", i, url);
		let maybe_client = if use_legacy_backend {
			let backend = subxt::backend::legacy::LegacyBackend::builder()
				.build(subxt::backend::rpc::RpcClient::new(helpers::client(url).await?));
			let maybe_client = OnlineClient::from_backend(Arc::new(backend)).await;
			maybe_client
		} else {
			let backend = subxt::backend::chain_head::ChainHeadBackend::builder()
				.transaction_timeout(6 * 3600)
				//note: This required new subxt release
				// subxt 0.42.1:
				// .submit_transactions_ignoring_follow_events()
				.build_with_background_driver(subxt::backend::rpc::RpcClient::new(
					helpers::client(url).await?,
				));

			let maybe_client = OnlineClient::from_backend(Arc::new(backend)).await;
			maybe_client
		};

		// let maybe_client = OnlineClient::<EthRuntimeConfig>::from_rpc_client(client);
		match maybe_client {
			Ok(client) => {
				info!("Connection established to: {}", url);
				return Ok(client);
			},
			Err(err) => {
				info!("API client {} error: {:?}", url, err);
				tokio::time::sleep(RETRY_DELAY).await;
			},
		};
	}

	let err = format!("Failed to connect to {} after {} attempts", url, MAX_ATTEMPTS);
	info!("{}", err);
	Err(err.into())
}
