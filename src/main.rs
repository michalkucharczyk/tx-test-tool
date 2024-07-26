use async_trait::async_trait;
use futures::stream::{self};
use futures_util::{stream::FuturesUnordered, StreamExt};
use parking_lot::RwLock;
use rand::Rng;
use std::{
    collections::HashSet,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::select;
use tracing::info;
use tracing_subscriber;

trait TransactionStatusIsTerminal {
    fn is_terminal(&self) -> bool;
}

mod fake_transaction;
mod fake_transaction_sink;
mod runner;

use fake_transaction::FakeTransaction;

#[derive(Debug, PartialEq, Clone)]
pub enum TransactionStatus<H> {
    Validated,
    Broadcasted,
    InBlock(H),
    Finalized(H),
    Dropped,
    Invalid,
    Error,
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

impl<H: Hash> TransactionStatusIsTerminal for TransactionStatus<H> {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Finalized(_) | Self::Dropped | Self::Invalid)
    }
}

pub enum ResbumitReason {
    Error,
}

pub enum TransactiondStoreStatus {
    Ready,
    InProgress,
    Done,
}

pub trait Hash: Send + Sync {}

pub trait Transaction<H: Hash>: Sync + Send {
    fn hash(&self) -> H;
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

/// Abstraction for RPC client
#[async_trait]
pub trait TransactionsSink<H: Hash, T: Transaction<H>>: Sync {
    async fn submit_and_watch(
        &self,
        tx: T,
    ) -> Result<StreamOf<TransactionStatus<H>>, Box<dyn std::error::Error>>;
    async fn submit(&self, tx: T) -> Result<H, Box<dyn std::error::Error>>;

    ///Current count of transactions being processed by sink
    fn count(&self) -> usize;
}

pub trait TransactionsStore<H: Hash, T: Transaction<H>>: Sync {
    fn ready(&self) -> impl IntoIterator<Item = T>;
}

#[async_trait]
pub trait ResubmitQueue<H: Hash, T: Transaction<H>>: Sync {
    async fn resubmit(tx: T, reason: ResbumitReason);
    fn stream_of_resubmits(&self) -> StreamOf<T>;
}

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
