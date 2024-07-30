//todo:
#![allow(dead_code)]
//todo:
#![allow(unused_imports)]
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
mod subxt_api_connector;
mod subxt_transaction;

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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Subxt error.
    #[error("subxt error: {0}")]
    Subxt(#[from] subxt::Error),
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

// pub enum TransactiondStoreStatus {
//     Ready,
//     InProgress,
//     Done,
// }

pub trait Transaction: Send + Sync {
    type HashType: BlockHash;
    fn hash(&self) -> Self::HashType;
    fn as_any(&self) -> &dyn Any;
}

pub trait ResubmitHandler: Sized {
    fn handle_resubmit_request(self) -> Option<Self>;
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

/// Abstraction for RPC client
#[async_trait]
pub trait TransactionsSink<H: BlockHash>: Send + Sync {
    async fn submit_and_watch(
        &self,
        tx: &dyn Transaction<HashType = H>,
    ) -> Result<StreamOf<TransactionStatus<H>>, Error>;

    async fn submit(&self, tx: &dyn Transaction<HashType = H>) -> Result<H, Error>;

    ///Current count of transactions being processed by sink
    fn count(&self) -> usize;
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
}
