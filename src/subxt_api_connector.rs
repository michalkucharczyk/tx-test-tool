use super::subxt_transaction::EthRuntimeConfig;
use std::{error::Error, sync::Arc, time::Duration};
use subxt::{backend::legacy::LegacyBackend, OnlineClient};
use tracing::info;

mod my_jsonrpsee_helpers {
	pub use jsonrpsee::{
		client_transport::ws::{self, EitherStream, Url, WsTransportClientBuilder},
		core::client::{Client, Error},
	};
	use tokio_util::compat::Compat;

	pub type Sender = ws::Sender<Compat<EitherStream>>;
	pub type Receiver = ws::Receiver<Compat<EitherStream>>;

	/// Build WS RPC client from URL
	pub async fn client(url: &str) -> Result<Client, Error> {
		let (sender, receiver) = ws_transport(url).await?;
		Ok(Client::builder()
			.max_buffer_capacity_per_subscription(4096)
			.max_concurrent_requests(128000)
			.build_with_tokio(sender, receiver))
	}

	async fn ws_transport(url: &str) -> Result<(Sender, Receiver), Error> {
		let url = Url::parse(url).map_err(|e| Error::Transport(e.into()))?;
		WsTransportClientBuilder::default()
			.build(url)
			.await
			.map_err(|e| Error::Transport(e.into()))
	}
}
/// Maximal number of connection attempts.
const MAX_ATTEMPTS: usize = 10;
/// Delay period between failed connection attempts.
const RETRY_DELAY: Duration = Duration::from_secs(1);
pub async fn connect<C: subxt::Config>(url: &str) -> Result<OnlineClient<C>, Box<dyn Error>> {
	for i in 0..MAX_ATTEMPTS {
		info!("Attempt #{}: Connecting to {}", i, url);
		// let maybe_client = OnlineClient::<EthRuntimeConfig>::from_url(url).await;
		let backend = LegacyBackend::builder()
			.build(subxt::backend::rpc::RpcClient::new(my_jsonrpsee_helpers::client(url).await?));
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
