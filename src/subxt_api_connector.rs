use std::{error::Error, sync::Arc, time::Duration};
use subxt::{backend::legacy::LegacyBackend, OnlineClient};
use tracing::info;

use crate::helpers;

const MAX_ATTEMPTS: usize = 10;
/// Delay period between failed connection attempts.
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Connect to a RPC node.
pub(crate) async fn connect<C: subxt::Config>(
	url: &str,
) -> Result<OnlineClient<C>, Box<dyn Error>> {
	for i in 0..MAX_ATTEMPTS {
		info!("Attempt #{}: Connecting to {}", i, url);
		// let maybe_client = OnlineClient::<EthRuntimeConfig>::from_url(url).await;
		let backend = LegacyBackend::builder()
			.build(subxt::backend::rpc::RpcClient::new(helpers::client(url).await?));
		let maybe_client = OnlineClient::from_backend(Arc::new(backend)).await;

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
