use std::{collections::HashMap, marker::PhantomData, pin::Pin};

use crate::transaction::TransactionMonitor;
use async_trait::async_trait;
use futures::Future;
use subxt::{blocks::Block, config::Hasher, OnlineClient};
use subxt_core::config::Header;
use tokio::{
	select,
	sync::{mpsc, oneshot},
};
use tracing::{info, trace};

const LOG_TARGET: &str = "block_monitor";

pub type BlockMonitorTask = Pin<Box<dyn Future<Output = ()> + Send>>;

type BlockNumber = u64;
pub type TxFoundListener<H> = oneshot::Receiver<H>;
type TxFoundListenerTrigger<H> = oneshot::Sender<H>;
type HashOf<C> = <C as subxt::Config>::Hash;

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
	pub async fn new(uri: &String) -> Self {
		trace!(uri, "BlockNumber::new");
		let api = OnlineClient::<C>::from_insecure_url(uri)
			.await
			.expect("should connect to rpc client");
		let (listener_request_tx, rx) = mpsc::channel(100);
		tokio::spawn(async { Self::run(api, rx).await });
		Self { listener_request_tx, _p: Default::default() }
	}

	async fn register_listener(&self, h: HashOf<C>) -> TxFoundListener<HashOf<C>> {
		trace!(hash = ?h, "register_listener");
		let (tx, external_listener) = oneshot::channel();
		self.listener_request_tx.send((h, tx)).await.unwrap();

		external_listener
	}

	async fn handle_block(
		callbacks: &mut HashMap<HashOf<C>, TxFoundListenerTrigger<HashOf<C>>>,
		block: Block<C, OnlineClient<C>>,
		finalized: bool,
	) -> Result<(), Box<dyn std::error::Error>> {
		let block_number: u64 = block.header().number().into();
		let block_hash = block.hash();

		let extrinsics = block.extrinsics().await?;
		let extrinsics_count = extrinsics.len();
		if finalized {
			for ext in extrinsics.iter() {
				let ext = ext?;
				let hash = <C as subxt::Config>::Hasher::hash_of(&ext.bytes());
				if let Some(trigger) = callbacks.remove(&hash) {
					trace!(?hash, "found transaction, notifying");
					trigger.send(block_hash).unwrap();
				}
			}
			info!(block_number, ?block_hash, extrinsics_count, "FINALIZED block");
		} else {
			info!(block_number, ?block_hash, extrinsics_count, "     BEST block");
		}
		Ok(())
	}

	async fn block_monitor_inner(
		api: OnlineClient<C>,
		mut listener_requrest_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger<HashOf<C>>)>,
	) -> Result<(), Box<dyn std::error::Error>> {
		let mut finalized_blocks_sub = api.blocks().subscribe_finalized().await?;
		let mut best_blocks_sub = api.blocks().subscribe_best().await?;

		let mut callbacks = HashMap::<HashOf<C>, TxFoundListenerTrigger<HashOf<C>>>::new();
		loop {
			select! {
				Some(Ok(block)) = finalized_blocks_sub.next() => {
					Self::handle_block(&mut callbacks, block, true).await?;
				}

				Some(Ok(block)) = best_blocks_sub.next() => {
					Self::handle_block(&mut callbacks, block, false).await?;
				}

				Some(listener_request) = listener_requrest_rx.recv() => {
					trace!("listener_request: {:?}", listener_request.0);
					callbacks.insert(listener_request.0, listener_request.1);
				}
			}
		}
	}

	pub async fn run(
		api: OnlineClient<C>,
		listener_requrest_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger<HashOf<C>>)>,
	) {
		let _ = Self::block_monitor_inner(api, listener_requrest_rx).await;
	}
}
