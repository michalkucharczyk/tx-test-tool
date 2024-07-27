use async_trait::async_trait;
use futures::stream::{self};
use futures_util::{stream::FuturesUnordered, StreamExt};
use parking_lot::RwLock;
use rand::Rng;
use std::{
    any::Any,
    collections::HashSet,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use subxt::{self, tx::TxStatus, OnlineClient};
use subxt_core::config::BlockHash;
use tokio::select;
use tracing::info;
use tracing_subscriber;

trait TransactionStatusIsTerminal {
    fn is_terminal(&self) -> bool;
    fn is_finalized(&self) -> bool;
    fn is_error(&self) -> bool;
}

mod fake_transaction;
mod fake_transaction_sink;
mod runner;
mod transaction_store;

use fake_transaction::FakeTransaction;

#[derive(Debug, PartialEq, Clone)]
pub enum TransactionStatus<H: BlockHash> {
    Validated,
    Broadcasted(u32),
    InBlock(H),
    NoLongerInBestBlock,
    Finalized(H),
    Dropped(String),
    Invalid(String),
    Error(String),
}

impl<C: subxt::Config> From<TxStatus<C, OnlineClient<C>>> for TransactionStatus<C::Hash> {
    fn from(value: TxStatus<C, OnlineClient<C>>) -> Self {
        match value {
            TxStatus::Validated => TransactionStatus::Validated,
            TxStatus::Broadcasted { num_peers } => TransactionStatus::Broadcasted(num_peers),
            TxStatus::InBestBlock(tx) => TransactionStatus::InBlock(tx.extrinsic_hash()),
            TxStatus::InFinalizedBlock(tx) => TransactionStatus::Finalized(tx.extrinsic_hash()),
            TxStatus::Error { message } => TransactionStatus::Error(message),
            TxStatus::Invalid { message } => TransactionStatus::Invalid(message),
            TxStatus::Dropped { message } => TransactionStatus::Dropped(message),
            TxStatus::NoLongerInBestBlock => TransactionStatus::NoLongerInBestBlock,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    // /// Codec error.
    // #[error("subxt error: {0}")]
    // Subxt(#[from] subxt::Error),
    /// Other error.
    #[error("Other error: {0}")]
    Other(String),
}

impl<H: BlockHash> TransactionStatusIsTerminal for TransactionStatus<H> {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finalized(_) | Self::Dropped(_) | Self::Invalid(_) | Self::Error(_)
        )
    }

    fn is_finalized(&self) -> bool {
        matches!(self, Self::Finalized(_))
    }

    fn is_error(&self) -> bool {
        matches!(self, Self::Dropped(_) | Self::Invalid(_) | Self::Error(_))
    }
}

// pub enum ResbumitReason {
//     Error,
// }

// pub enum TransactiondStoreStatus {
//     Ready,
//     InProgress,
//     Done,
// }

pub trait Transaction: Sync {
    type HashType: BlockHash;
    fn hash(&self) -> Self::HashType;
    fn as_any(&self) -> &dyn Any;
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

/// Abstraction for RPC client
#[async_trait]
pub trait TransactionsSink<H: BlockHash>: Sync {
    async fn submit_and_watch(
        &self,
        tx: &dyn Transaction<HashType = H>,
    ) -> Result<StreamOf<TransactionStatus<H>>, Box<dyn std::error::Error + Send>>;
    async fn submit(
        &self,
        tx: &dyn Transaction<HashType = H>,
    ) -> Result<H, Box<dyn std::error::Error + Send>>;

    ///Current count of transactions being processed by sink
    fn count(&self) -> usize;
}

// pub trait TransactionsStore<H: BlockHash, T: Transaction<H>>: Sync {
//     fn ready(&self) -> impl IntoIterator<Item = >;
// }

// #[async_trait]
// pub trait ResubmitQueue<H: Hash, T: Transaction<H>>: Sync {
//     async fn resubmit(tx: T, reason: ResbumitReason);
//     fn stream_of_resubmits(&self) -> StreamOf<T>;
// }

////////////////////////////////////////////////////////////////////////////////

async fn send_txs(i: usize) -> (usize, u64, Instant) {
    let delay = rand::thread_rng().gen_range(1000..10000);
    info!("send_txs start: {i} / {delay}");
    let start = Instant::now();
    tokio::time::sleep(Duration::from_millis(delay)).await;
    info!("send_txs done: {i} / {delay}");
    (i, delay, start)
}

async fn run() {
    let mut workers = FuturesUnordered::new();
    let mut cnt = 0;

    for i in 0..10000 {
        workers.push(send_txs(i));
        cnt = i;
    }

    loop {
        select! {
            done = workers.next() => {
                match done {
                    Some((i, delay, started)) => {
                        cnt = cnt + 1;
                        let elapsed = started.elapsed();
                        let diff = elapsed.as_millis() - delay as u128;
                        info!("FINISHED {i} slept:{delay} actual:{elapsed:?} diff:{diff} => send_txs push: {cnt} / {}", workers.len());
                        workers.push(send_txs(cnt));
                    }
                    None => {
                        info!("done");
                        break;
                    }
                }
            }
        }
    }
}

fn init_logger() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // tracing_subscriber::fmt().with_max_level(Level::INFO).init();
        let timer = tracing_subscriber::fmt::time::OffsetTime::new(
            time::macros::offset!(+2),
            time::macros::format_description!(
                "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"
            ),
        );
        tracing_subscriber::fmt().with_timer(timer).init();
    });
}

#[tokio::main]
async fn main() {
    init_logger();
    info!("Hello, world!");
    run().await;
}
