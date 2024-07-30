//todo:
#![allow(dead_code)]
//todo:
#![allow(unused_imports)]
use futures::future::join;
use jsonrpsee::types::ErrorObject;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};

use self::fake_transaction::{FakeHash, FakeTransaction};
use self::fake_transaction_sink::FakeTransactionSink;

use super::*;

#[derive(Debug)]
enum ExecutionEvent<H: BlockHash> {
    Popped(Instant),
    Sent(Instant),
    Resubmitted(Instant),
    SubmitResult(Instant, Result<(), Error>),
    SubmitAndWatchResult(Instant, Result<(), Error>),
    TxPoolEvent(Instant, TransactionStatus<H>),
}

impl<H: BlockHash> ExecutionEvent<H> {
    fn popped() -> Self {
        Self::Popped(Instant::now())
    }
    fn sent() -> Self {
        Self::Sent(Instant::now())
    }
    fn submit_and_watch_result(r: Result<(), Error>) -> Self {
        Self::SubmitAndWatchResult(Instant::now(), r)
    }
    fn submit_result(r: Result<(), Error>) -> Self {
        Self::SubmitResult(Instant::now(), r)
    }
}

impl<H: BlockHash> From<TransactionStatus<H>> for ExecutionEvent<H> {
    fn from(value: TransactionStatus<H>) -> Self {
        Self::TxPoolEvent(Instant::now(), value)
    }
}

/// should contain account metadata from sending tool perspecive, e.g. //{}//{idx} used to generate account, or
/// alice/bob maybe call etc...
struct AccountMetadata {}

trait ExecutionLog: Sync + Send {
    type HashType: BlockHash;

    fn push_event(&self, event: ExecutionEvent<Self::HashType>);

    // all methods used for generating stats:
    fn hash(&self) -> Self::HashType;
    fn nonce(&self) -> u128;
    fn account_metadata(&self) -> AccountMetadata;

    fn in_blocks(&self) -> Option<Vec<Self::HashType>>;
    fn finalized(&self) -> Option<Self::HashType>;
    fn is_watched(&self) -> bool;

    fn time_to_finalized(&self) -> Option<Duration>;
    fn times_to_inblock(&self) -> Option<Vec<Duration>>;
    fn time_to_dropped(&self) -> Option<Duration>;
    fn time_to_invalid(&self) -> Option<Duration>;

    fn get_invalid_reason(&self) -> Option<String>;
    fn get_error_reason(&self) -> Option<String>;
    fn get_dropped_reason(&self) -> Option<String>;
    fn get_resent_count(&self) -> u32;
}

#[derive(Debug)]
struct DefaultExecutionLog<H: BlockHash> {
    events: RwLock<Vec<ExecutionEvent<H>>>,
}
impl<H: BlockHash> Default for DefaultExecutionLog<H> {
    fn default() -> Self {
        Self {
            events: Default::default(),
        }
    }
}

impl<H: BlockHash> ExecutionLog for DefaultExecutionLog<H> {
    type HashType = H;

    fn push_event(&self, event: ExecutionEvent<Self::HashType>) {
        info!(?event, "push_event:");
        self.events.write().push(event);
    }

    // all methods used for generating stats:
    fn hash(&self) -> Self::HashType {
        unimplemented!()
    }
    fn nonce(&self) -> u128 {
        unimplemented!()
    }
    fn account_metadata(&self) -> AccountMetadata {
        unimplemented!()
    }

    fn in_blocks(&self) -> Option<Vec<Self::HashType>> {
        unimplemented!()
    }
    fn finalized(&self) -> Option<Self::HashType> {
        unimplemented!()
    }
    fn is_watched(&self) -> bool {
        unimplemented!()
    }

    fn time_to_finalized(&self) -> Option<Duration> {
        unimplemented!()
    }
    fn times_to_inblock(&self) -> Option<Vec<Duration>> {
        unimplemented!()
    }
    fn time_to_dropped(&self) -> Option<Duration> {
        unimplemented!()
    }
    fn time_to_invalid(&self) -> Option<Duration> {
        unimplemented!()
    }

    fn get_invalid_reason(&self) -> Option<String> {
        unimplemented!()
    }
    fn get_error_reason(&self) -> Option<String> {
        unimplemented!()
    }
    fn get_dropped_reason(&self) -> Option<String> {
        unimplemented!()
    }
    fn get_resent_count(&self) -> u32 {
        unimplemented!()
    }
}

#[derive(Debug)]
pub enum ResubmitReason {
    Dropped,
    Mortality,
    RpcError,
}

enum ExecutionResult<T: TxTask> {
    NeedsResubmit(ResubmitReason, T),
    Error(TxHash<T>),
    Done(TxHash<T>),
}

impl<T: TxTask> std::fmt::Debug for ExecutionResult<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsResubmit(r, t) => write!(f, "NeedsResubmit {r:?} {:?}", t.tx().hash()),
            Self::Error(h) => write!(f, "Error {h:?}"),
            Self::Done(h) => write!(f, "Doneh {h:?}"),
        }
    }
}

trait NeedsResubmit {
    fn needs_resubmission(&self) -> Option<ResubmitReason>;
}

impl<H: BlockHash> NeedsResubmit for TransactionStatus<H> {
    fn needs_resubmission(&self) -> Option<ResubmitReason> {
        matches!(self, TransactionStatus::Dropped(_)).then_some(ResubmitReason::Dropped)
    }
}

impl NeedsResubmit for Error {
    fn needs_resubmission(&self) -> Option<ResubmitReason> {
        if let Error::Subxt(subxt::Error::Rpc(subxt::error::RpcError::ClientError(ref o))) = self {
            if let Some(eo) = o.source() {
                let code = eo.downcast_ref::<ErrorObject>().unwrap().code();
                //polkdot-sdk/substrate/client/rpc-api/author -> POOL_IMMEDIATELY_DROPPED
                if code == 1016 {
                    return Some(ResubmitReason::Dropped);
                }
            }
        }
        return None;
    }
}

#[async_trait]
pub trait TxTask: Send + Sync + Sized {
    type HashType: BlockHash + 'static;
    type Transaction: Transaction + ResubmitHandler;

    fn tx(&self) -> &Self::Transaction;
    fn is_watched(&self) -> bool;

    async fn send_watched_tx(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self>;

    async fn send_tx(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self>;

    async fn execute(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self>;

    fn handle_resubmit_request(self) -> Option<Self>;
}

struct DefaultTxTask<T>
where
    T: Transaction,
    <T as Transaction>::HashType: 'static,
{
    tx: T,
    watched: bool,
}

#[async_trait]
impl<H: BlockHash, T: Transaction<HashType = H> + ResubmitHandler + Send> TxTask
    for DefaultTxTask<T>
{
    type HashType = H;
    type Transaction = T;

    fn tx(&self) -> &T {
        &self.tx
    }

    fn is_watched(&self) -> bool {
        self.watched
    }

    fn handle_resubmit_request(mut self) -> Option<Self> {
        if let Some(new_tx) = self.tx.handle_resubmit_request() {
            self.tx = new_tx;
            Some(self)
        } else {
            None
        }
    }

    async fn send_watched_tx(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self> {
        log.push_event(ExecutionEvent::sent());
        match rpc.submit_and_watch(self.tx()).await {
            Ok(mut stream) => {
                log.push_event(ExecutionEvent::submit_and_watch_result(Ok(())));
                while let Some(status) = stream.next().await {
                    info!("{status:?}");
                    log.push_event(status.clone().into());
                    if status.is_finalized() {
                        return ExecutionResult::Done(self.tx().hash());
                    } else if let Some(reason) = status.needs_resubmission() {
                        return ExecutionResult::NeedsResubmit(reason, self);
                    } else if status.is_error() {
                        return ExecutionResult::Error(self.tx().hash());
                    } else {
                        continue;
                    }
                }
                ExecutionResult::Error(self.tx().hash())
            }
            Err(e) => {
                let result = if let Some(reason) = e.needs_resubmission() {
                    ExecutionResult::NeedsResubmit(reason, self)
                } else {
                    ExecutionResult::Error(self.tx().hash())
                };
                log.push_event(ExecutionEvent::submit_and_watch_result(Err(e)));
                result
            }
        }
    }

    async fn send_tx(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self> {
        log.push_event(ExecutionEvent::sent());
        match rpc.submit(self.tx()).await {
            Ok(_) => {
                log.push_event(ExecutionEvent::submit_result(Ok(())));
                ExecutionResult::Done(self.tx().hash())
            }
            Err(e) => {
                let result = if let Some(reason) = e.needs_resubmission() {
                    ExecutionResult::NeedsResubmit(reason, self)
                } else {
                    ExecutionResult::Error(self.tx().hash())
                };
                log.push_event(ExecutionEvent::submit_result(Err(e)));
                result
            }
        }
    }

    async fn execute(
        self: Self,
        log: Arc<dyn ExecutionLog<HashType = Self::HashType>>,
        rpc: Arc<dyn TransactionsSink<Self::HashType>>,
    ) -> ExecutionResult<Self> {
        if self.is_watched() {
            self.send_watched_tx(log, rpc).await
        } else {
            self.send_tx(log, rpc).await
        }
    }
}

// pub trait TransactionsStore<H: BlockHash, T: Transaction<H>>: Sync {
//     fn ready(&self) -> impl IntoIterator<Item = >;
// }

trait TxTaskStore {
    type HashType: BlockHash;
    type Transaction: Transaction<HashType = Self::HashType>;
    fn pop() -> Option<Self::Transaction>;
}

type FakeTxTask = DefaultTxTask<FakeTransaction>;

impl<T: Transaction> DefaultTxTask<T> {
    fn new_watched(tx: T) -> Self {
        Self { tx, watched: true }
    }
    fn new_unwatched(tx: T) -> Self {
        Self { tx, watched: false }
    }
}

#[async_trait]
pub trait ResubmissionQueue<T: TxTask>: Default {
    async fn resubmit(&self, hash: T, reason: ResubmitReason);
    fn pop(&self) -> Option<T>;
    async fn run(&self);
    fn terminate(&self);
    async fn is_empty(&self) -> bool;
}

struct DefaultResubmissionQueue<T: TxTask> {
    waiting_queue: Arc<
        tokio::sync::RwLock<
            FuturesUnordered<Pin<Box<dyn futures::Future<Output = Option<T>> + Sync + Send>>>,
        >,
    >,
    ready_queue: Arc<RwLock<Vec<T>>>,
    terminate: Arc<AtomicBool>,
}

impl<T: TxTask> Clone for DefaultResubmissionQueue<T> {
    fn clone(&self) -> Self {
        Self {
            waiting_queue: self.waiting_queue.clone(),
            ready_queue: self.ready_queue.clone(),
            terminate: self.terminate.clone(),
        }
    }
}

impl<T: TxTask> Default for DefaultResubmissionQueue<T> {
    fn default() -> Self {
        Self {
            waiting_queue: Default::default(),
            ready_queue: Default::default(),
            terminate: AtomicBool::new(false).into(),
        }
    }
}

impl<T: TxTask> DefaultResubmissionQueue<T> {
    async fn wait(t: T) -> Option<T> {
        info!(">>>> B wait {:?}", t.tx().hash());
        tokio::time::sleep(Duration::from_millis(3000)).await;
        info!(">>>> A wait {:?}", t.tx().hash());
        t.handle_resubmit_request()
    }
}

#[async_trait]
impl<T: TxTask + 'static> ResubmissionQueue<T> for DefaultResubmissionQueue<T> {
    fn terminate(&self) {
        info!(">>>> TERMINATE ");
        self.terminate.store(true, Ordering::Relaxed);
    }

    async fn is_empty(&self) -> bool {
        self.waiting_queue.write().await.len() == 0 && self.ready_queue.write().is_empty()
    }

    async fn resubmit(&self, task: T, reason: ResubmitReason) {
        // todo!()
        // task.resubmit()
        // resubmitted.
        info!(">>>> B PUSH {}", self.waiting_queue.write().await.len());
        self.waiting_queue
            .write()
            .await
            .push(Box::pin(Self::wait(task)));
        info!(">>>> A PUSH {}", self.waiting_queue.write().await.len());
    }

    fn pop(&self) -> Option<T> {
        self.ready_queue.write().pop()
    }

    async fn run(&self) {
        loop {
            info!(">>>> RUN ");
            let mut q = self.waiting_queue.write().await;
            if q.len() == 0 {
                if self.terminate.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            } else {
                info!(">>>> RUN {}", q.len());
                while let Some(t) = q.next().await {
                    if let Some(t) = t {
                        info!(">>>> RUN {:?}", t.tx().hash());
                        self.ready_queue.write().push(t)
                    }
                }
            }
        }
    }
}
type TxHash<T> = <<T as TxTask>::Transaction as Transaction>::HashType;
struct Runner<T: TxTask, Sink: TransactionsSink<T::HashType>, Queue: ResubmissionQueue<T>> {
    logs: HashMap<TxHash<T>, Arc<DefaultExecutionLog<T::HashType>>>,
    transactions: Vec<T>,
    done: Vec<TxHash<T>>,
    errors: Vec<TxHash<T>>,
    rpc: Arc<Sink>,
    resubmission_queue: Queue,
}

// trait XXXX<H: BlockHash>: TxTask<HashType = H> + 'static + Sized {}
// impl<T, H: BlockHash> XXXX<H> for T where T: TxTask<HashType = H> + 'static + Sized {}
type FakeTxRunner = Runner<FakeTxTask, FakeTransactionSink, DefaultResubmissionQueue<FakeTxTask>>;

impl<T: TxTask, Sink, Queue: ResubmissionQueue<T>> Runner<T, Sink, Queue>
where
    Sink: TransactionsSink<T::HashType> + 'static,
    <T as runner::TxTask>::HashType: 'static,
{
    fn new(rpc: Sink, transactions: Vec<T>, resubmission_queue: Queue) -> Self {
        let logs = transactions
            .iter()
            .map(|t| (t.tx().hash(), DefaultExecutionLog::default().into()))
            .collect();

        Self {
            logs,
            transactions: transactions.into(),
            rpc: rpc.into(),
            resubmission_queue,
            done: Default::default(),
            errors: Default::default(),
        }
    }

    fn pop(&mut self) -> Option<T> {
        self.resubmission_queue
            .pop()
            .or_else(|| self.transactions.pop())
    }

    // pub async fn run_poc3(&mut self) {
    //     join(self.resubmission_queue.run(), self.run_poc2()).await;
    // }

    pub async fn run_poc2(&mut self) {
        let mut workers = FuturesUnordered::new();

        let mut i = 0;
        for _ in 0..5 {
            if let Some(t) = self.transactions.pop() {
                let t = Box::new(t);
                let log = self.logs[&t.tx().hash()].clone();
                log.push_event(ExecutionEvent::popped());
                // let log: Arc<dyn ExecutionLog<HashType = T::HashType>> = log;
                workers.push(t.execute(log, self.rpc.clone()));
                i = i + 1;
            } else {
                break;
            }
        }

        loop {
            select! {
                done = workers.next() => {
                    // info!(?done, workers_len=workers.len(), "DONE");
                    match done {
                        Some(result) => {
                            let task = {
                                self.pop()
                            };
                            if let Some(task) = task {
                                let log = self.logs[&task.tx().hash()].clone();
                                log.push_event(ExecutionEvent::popped());
                                workers.push(task.execute(log, self.rpc.clone()));
                            }
                            info!(?result, workers_len=workers.len(), "FINISHED");

                            match result {
                                ExecutionResult::NeedsResubmit(reason, t) => {
                                    self.resubmission_queue.resubmit(t, reason).await;
                                },
                                ExecutionResult::Done(hash) => {
                                    self.done.push(hash)
                                },
                                ExecutionResult::Error(hash) => {
                                    self.errors.push(hash)
                                }

                            }
                        }
                        None => {
                            tokio::time::sleep(Duration::from_millis(1000)).await;
                            self.resubmission_queue.terminate();
                            info!("done");
                            if self.resubmission_queue.is_empty().await {
                                break;
                            }
                            let task = {
                                self.pop()
                            };
                            if let Some(task) = task {
                                let log = self.logs[&task.tx().hash()].clone();
                                log.push_event(ExecutionEvent::popped());
                                workers.push(task.execute(log, self.rpc.clone()));
                            }
                            // break;
                        }
                    }
                }
            }
        }
        info!("logs {:#?}", self.logs);
    }
}

#[cfg(test)]
mod tests {
    use self::subxt_transaction::TransactionsSinkSubxt;

    use super::subxt_transaction::{EthRuntimeConfig, TransactionEth};
    use super::*;
    use subxt::config::substrate::SubstrateExtrinsicParamsBuilder as Params;
    use subxt::dynamic::Value;
    use subxt_signer::eth::dev;
    // #[tokio::test]
    // async fn oh_god() {
    //     init_logger();
    //
    //     let rpc = FakeTransactionSink::new();
    //     let mut transactions = (0..10)
    //         .map(|i| FakeTxTask::new_watched(FakeTransaction::new_finalizable_quick(i)))
    //         // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
    //         .collect::<Vec<_>>();
    //
    //     transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_invalid(
    //         11u32, 300,
    //     )));
    //     transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_error(
    //         12u32, 300,
    //     )));
    //
    //     let mut r = Runner::<
    //         DefaultTxTask<FakeTransaction>,
    //         FakeTransactionSink,
    //         DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
    //     >::new(rpc, transactions);
    //     r.run_poc2().await;
    // }
    //
    // type SubxtEthTxTask = DefaultTxTask<TransactionEth>;
    //
    // fn make_subxt_transaction(api: &OnlineClient<EthRuntimeConfig>, nonce: u64) -> TransactionEth {
    //     let alith = dev::alith();
    //     let baltathar = dev::baltathar();
    //
    //     let nonce = nonce;
    //     let tx_params = Params::new().nonce(nonce).build();
    //
    //     // let tx_call = subxt::dynamic::tx("System", "remark", vec![Value::from_bytes("heeelooo")]);
    //     let tx_call = subxt::dynamic::tx(
    //         "Balances",
    //         "transfer_keep_alive",
    //         vec![
    //             // // Substrate:
    //             // Value::unnamed_variant("Id", [Value::from_bytes(receiver.public())]),
    //             // Eth:
    //             Value::unnamed_composite(vec![Value::from_bytes(alith.account_id())]),
    //             Value::u128(1u32.into()),
    //         ],
    //     );
    //
    //     let tx: TransactionEth = api
    //         .tx()
    //         .create_signed_offline(&tx_call, &baltathar, tx_params)
    //         .unwrap();
    //
    //     info!("tx hash: {:?}", tx.hash());
    //
    //     tx
    // }
    //
    // #[tokio::test]
    // async fn oh_god2() {
    //     init_logger();
    //
    //     // let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
    //     //     .await
    //     //     .unwrap();
    //     let api = subxt_api_connector::connect("ws://127.0.0.1:9933")
    //         .await
    //         .unwrap();
    //
    //     let rpc = TransactionsSinkSubxt::<EthRuntimeConfig>::new();
    //
    //     let transactions = (0..300000)
    //         .map(|i| SubxtEthTxTask::new_watched(make_subxt_transaction(&api, i + 60000)))
    //         .rev()
    //         // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
    //         .collect::<Vec<_>>();
    //
    //     let mut r = Runner::<
    //         DefaultTxTask<TransactionEth>,
    //         TransactionsSinkSubxt<EthRuntimeConfig>,
    //         DefaultResubmissionQueue<DefaultTxTask<TransactionEth>>,
    //     >::new(rpc, transactions);
    //     r.run_poc2().await;
    // }

    #[tokio::test]
    async fn resubmit() {
        init_logger();

        let rpc = FakeTransactionSink::new();
        let mut transactions = (0..10)
            .map(|i| FakeTxTask::new_watched(FakeTransaction::new_droppable_2nd_success(i, i * 5)))
            // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
            .collect::<Vec<_>>();

        let queue = DefaultResubmissionQueue::default();

        let mut r = Runner::<
            DefaultTxTask<FakeTransaction>,
            FakeTransactionSink,
            DefaultResubmissionQueue<DefaultTxTask<FakeTransaction>>,
        >::new(rpc, transactions, queue.clone());
        join(queue.run(), r.run_poc2()).await;
    }
}
