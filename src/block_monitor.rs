// Copyright (C) Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use std::{collections::HashMap, marker::PhantomData, pin::Pin};

use crate::transaction::TransactionMonitor;
use async_trait::async_trait;
use clap::ValueEnum;
use futures::Future;
use subxt::{blocks::Block, OnlineClient};
use subxt_core::config::Header;
use tokio::{
	select,
	sync::{mpsc, oneshot},
};
use tracing::{info, trace};

/// Monitors if transactions are part of finalized blocks and notifies external listeners when
/// found.
pub type BlockMonitorTask = Pin<Box<dyn Future<Output = ()> + Send>>;

type TxFoundListener<H> = oneshot::Receiver<H>;
type TxFoundListenerTrigger<H> = oneshot::Sender<H>;
type HashOf<C> = <C as subxt::Config>::Hash;

#[derive(ValueEnum, Copy, Clone, Debug)]
pub enum BlockMonitorDisplayOptions {
	Best,
	Finalized,
	All,
}

impl BlockMonitorDisplayOptions {
	fn display_best(&self) -> bool {
		matches!(self, Self::Best | Self::All)
	}
	fn display_finalized(&self) -> bool {
		matches!(self, Self::Finalized | Self::All)
	}
}

#[derive(Clone)]
pub struct BlockMonitor<C: subxt::Config> {
	listener_request_tx: mpsc::Sender<(HashOf<C>, TxFoundListenerTrigger<HashOf<C>>)>,
	_p: PhantomData<C>,
}

#[async_trait]
impl<C: subxt::Config> TransactionMonitor<HashOf<C>> for BlockMonitor<C> {
	async fn wait(&self, tx_hash: HashOf<C>) -> HashOf<C> {
		let listener = self.register_listener(tx_hash).await;
		listener.await.unwrap()
	}
}

impl<C: subxt::Config> BlockMonitor<C> {
	/// Instantiates a [`BlockMonitor`].
	pub async fn new(uri: &str) -> Self {
		trace!(uri, "BlockNumber::new");
		let api = OnlineClient::<C>::from_insecure_url(uri)
			.await
			.expect("should connect to rpc client");
		let (listener_request_tx, rx) = mpsc::channel(100);
		tokio::spawn(async { Self::run(api, rx, BlockMonitorDisplayOptions::All).await });
		Self { listener_request_tx, _p: Default::default() }
	}

	pub async fn new_with_options(uri: &str, options: BlockMonitorDisplayOptions) -> Self {
		trace!(uri, "BlockNumber::new");
		let api = OnlineClient::<C>::from_insecure_url(uri)
			.await
			.expect("should connect to rpc client");
		let (listener_request_tx, rx) = mpsc::channel(100);
		tokio::spawn(async move { Self::run(api, rx, options).await });
		Self { listener_request_tx, _p: Default::default() }
	}

	/// Returns the receiving end of a channel where a notification is sent if the transaction with
	/// the given hash is found in a finalized block.
	pub async fn register_listener(&self, h: HashOf<C>) -> TxFoundListener<HashOf<C>> {
		trace!(hash = ?h, "register_listener");
		let (tx, external_listener) = oneshot::channel();
		self.listener_request_tx.send((h, tx)).await.unwrap();

		external_listener
	}

	async fn handle_block(
		callbacks: &mut HashMap<HashOf<C>, TxFoundListenerTrigger<HashOf<C>>>,
		block: Block<C, OnlineClient<C>>,
		options: BlockMonitorDisplayOptions,
		finalized: bool,
	) -> Result<(), Box<dyn std::error::Error>> {
		let block_number: u64 = block.header().number().into();
		let block_hash = block.hash();

		let extrinsics = block.extrinsics().await?;
		let extrinsics_count = extrinsics.len();
		if finalized {
			for ext in extrinsics.iter() {
				let hash = ext.hash();
				if let Some(trigger) = callbacks.remove(&hash) {
					trace!(?hash, "found transaction, notifying");
					trigger.send(block_hash).unwrap();
				}
			}
			if options.display_finalized() {
				info!(block_number, extrinsics_count, "FINALIZED block");
			}
		} else if options.display_best() {
			info!(block_number, extrinsics_count, "     BEST block");
		}
		Ok(())
	}

	async fn block_monitor_inner(
		api: OnlineClient<C>,
		mut listener_request_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger<HashOf<C>>)>,
		options: BlockMonitorDisplayOptions,
	) -> Result<(), Box<dyn std::error::Error>> {
		let mut finalized_blocks_sub = api.blocks().subscribe_finalized().await?;
		let mut best_blocks_sub = api.blocks().subscribe_best().await?;

		let mut callbacks = HashMap::<HashOf<C>, TxFoundListenerTrigger<HashOf<C>>>::new();
		loop {
			select! {
				Some(Ok(block)) = finalized_blocks_sub.next() => {
					Self::handle_block(&mut callbacks, block, options, true).await?;
				}

				Some(Ok(block)) = best_blocks_sub.next() => {
					Self::handle_block(&mut callbacks, block, options, false).await?;
				}

				Some((hash, tx)) = listener_request_rx.recv() => {
					trace!("listener_request: {:?}", hash);
					callbacks.insert(hash, tx);
				}
			}
		}
	}

	async fn run(
		api: OnlineClient<C>,
		listener_requrest_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger<HashOf<C>>)>,
		options: BlockMonitorDisplayOptions,
	) {
		let _ = Self::block_monitor_inner(api, listener_requrest_rx, options).await;
	}
}
