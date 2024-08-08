use std::{collections::HashMap, marker::PhantomData, pin::Pin, time::Instant};

use futures::Future;
use subxt::{blocks::Block, config::Hasher, OnlineClient};
use subxt_core::config::Header;
use tokio::{
	select,
	sync::{mpsc, oneshot},
};
use tracing::info;

pub type BlockMonitorTask = Pin<Box<dyn Future<Output = ()> + Send>>;

type BlockNumber = u64;
pub type TxFoundListener = oneshot::Receiver<BlockNumber>;
type TxFoundListenerTrigger = oneshot::Sender<BlockNumber>;
type HashOf<C> = <C as subxt::Config>::Hash;

pub struct BlockMonitor<C: subxt::Config> {
	listener_request_tx: mpsc::Sender<(HashOf<C>, TxFoundListenerTrigger)>,
	_p: PhantomData<C>,
}

impl<C: subxt::Config> BlockMonitor<C> {
	pub async fn new(uri: String) -> (Self, BlockMonitorTask) {
		let api = OnlineClient::<C>::from_insecure_url(uri)
			.await
			.expect("should connect to rpc client");
		let (listener_request_tx, rx) = mpsc::channel(100);
		(Self { listener_request_tx, _p: Default::default() }, Box::pin(Self::run(api, rx)))
	}

	async fn handle_block(
		callbacks: &mut HashMap<HashOf<C>, TxFoundListenerTrigger>,
		block: Block<C, OnlineClient<C>>,
	) -> Result<(), Box<dyn std::error::Error>> {
		let start = Instant::now();

		let block_number: u64 = block.header().number().into();
		let block_hash = block.hash();

		info!("Block #{block_number}:");
		info!("  Hash: {block_hash:?}");
		// info!("  Extrinsics:");

		// Log each of the extrinsic with it's associated events:
		let extrinsics = block.extrinsics().await?;
		let extrinsics_len = extrinsics.len();
		for ext in extrinsics.iter() {
			let ext = ext?;
			let hash = <C as subxt::Config>::Hasher::hash_of(&ext.bytes());
			if let Some(trigger) = callbacks.remove(&hash) {
				trigger.send(block_number).unwrap();
			}
		}

		info!("  Extrinsics: {} {:?}", extrinsics_len, start.elapsed());
		Ok(())
	}

	async fn block_monitor_inner(
		api: OnlineClient<C>,
		mut listener_requrest_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger)>,
	) -> Result<(), Box<dyn std::error::Error>> {
		let mut blocks_sub = api.blocks().subscribe_finalized().await?;

		let mut callbacks = HashMap::<HashOf<C>, TxFoundListenerTrigger>::new();
		loop {
			select! {
				Some(Ok(block)) = blocks_sub.next() => {
					Self::handle_block(&mut callbacks, block).await?;
				}

				Some(listener_request) = listener_requrest_rx.recv() => {
					println!("listener_request: {:?}", listener_request.0);
					callbacks.insert(listener_request.0, listener_request.1);
				}
			}
		}
	}

	pub async fn run(
		api: OnlineClient<C>,
		listener_requrest_rx: mpsc::Receiver<(HashOf<C>, TxFoundListenerTrigger)>,
	) {
		let _ = Self::block_monitor_inner(api, listener_requrest_rx).await;
	}
}
