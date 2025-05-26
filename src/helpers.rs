// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

pub use jsonrpsee::{
	client_transport::ws::{self, EitherStream, Url, WsTransportClientBuilder},
	core::client::{Client, Error},
};
use std::{pin::Pin, time::Duration};
use tokio_util::compat::Compat;

/// Helper type for a futures stream.
pub(crate) type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

/// Type alias for a websocket sender.
pub(crate) type Sender = ws::Sender<Compat<EitherStream>>;
/// Type alias for a websocket receiver.
pub(crate) type Receiver = ws::Receiver<Compat<EitherStream>>;

/// Build WS RPC client from URL
pub(crate) async fn client(url: &str) -> Result<Client, Error> {
	let (sender, receiver) = ws_transport(url).await?;
	Ok(Client::builder()
		.max_buffer_capacity_per_subscription(4096)
		.max_concurrent_requests(1280000)
		.build_with_tokio(sender, receiver))
}

async fn ws_transport(url: &str) -> Result<(Sender, Receiver), Error> {
	let url = Url::parse(url).map_err(|e| Error::Transport(e.into()))?;
	WsTransportClientBuilder::default()
		.max_request_size(400 * 1024 * 1024)
		.max_response_size(400 * 1024 * 1024)
		.connection_timeout(Duration::from_secs(600))
		.build(url)
		.await
		.map_err(|e| Error::Transport(e.into()))
}
