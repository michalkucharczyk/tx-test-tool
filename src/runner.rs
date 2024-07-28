use jsonrpsee::types::ErrorObject;
use std::collections::HashMap;

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

trait ExecutionLog: Sync {
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

enum ExecutionResult<H: BlockHash> {
    NeedsResubmit(Box<dyn TxTask<HashType = H>>),
    Error,
    Done,
}

impl<H: BlockHash> std::fmt::Debug for ExecutionResult<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsResubmit(t) => write!(f, "NeedsResubmit {:?}", t.tx().hash()),
            Self::Error => write!(f, "Error"),
            Self::Done => write!(f, "Done"),
        }
    }
}

trait NeedsResubmit {
    fn needs_resubmission(&self) -> bool;
}

impl<H: BlockHash> NeedsResubmit for TransactionStatus<H> {
    fn needs_resubmission(&self) -> bool {
        matches!(self, TransactionStatus::Dropped(_))
    }
}

impl NeedsResubmit for Error {
    fn needs_resubmission(&self) -> bool {
        if let Error::Subxt(subxt::Error::Rpc(subxt::error::RpcError::ClientError(ref o))) = self {
            if let Some(eo) = o.source() {
                let code = eo.downcast_ref::<ErrorObject>().unwrap().code();
                //polkdot-sdk/substrate/client/rpc-api/author -> POOL_IMMEDIATELY_DROPPED
                if code == 1016 {
                    return true;
                }
            }
        }
        return false;
    }
}

#[async_trait]
trait TxTask {
    type HashType: BlockHash;
    fn tx(&self) -> &dyn Transaction<HashType = Self::HashType>;
    fn is_watched(&self) -> bool;

    async fn send_watched_tx(
        self: Box<Self>,
        log: &dyn ExecutionLog<HashType = Self::HashType>,
        rpc: &dyn TransactionsSink<Self::HashType>,
    ) -> ExecutionResult<Self::HashType>
    where
        Self: Sized + 'static,
    {
        log.push_event(ExecutionEvent::sent());
        match rpc.submit_and_watch(self.tx()).await {
            Ok(mut stream) => {
                log.push_event(ExecutionEvent::submit_and_watch_result(Ok(())));
                while let Some(status) = stream.next().await {
                    log.push_event(status.clone().into());
                    if status.is_finalized() {
                        return ExecutionResult::Done;
                    } else if status.needs_resubmission() {
                        return ExecutionResult::NeedsResubmit(self);
                    } else if status.is_error() {
                        return ExecutionResult::Error;
                    } else {
                        continue;
                    }
                }
                ExecutionResult::Error
            }
            Err(e) => {
                let result = if e.needs_resubmission() {
                    ExecutionResult::NeedsResubmit(self)
                } else {
                    ExecutionResult::Error
                };
                log.push_event(ExecutionEvent::submit_and_watch_result(Err(e)));
                result
            }
        }
    }

    async fn send_tx(
        self: Box<Self>,
        log: &dyn ExecutionLog<HashType = Self::HashType>,
        rpc: &dyn TransactionsSink<Self::HashType>,
    ) -> ExecutionResult<Self::HashType>
    where
        Self: Sized + 'static,
    {
        log.push_event(ExecutionEvent::sent());
        match rpc.submit(self.tx()).await {
            Ok(_) => {
                log.push_event(ExecutionEvent::submit_result(Ok(())));
                ExecutionResult::Done
            }
            Err(e) => {
                let result = if e.needs_resubmission() {
                    ExecutionResult::NeedsResubmit(self)
                } else {
                    ExecutionResult::Error
                };
                log.push_event(ExecutionEvent::submit_result(Err(e)));
                result
            }
        }
    }

    async fn execute(
        self: Box<Self>,
        log: &dyn ExecutionLog<HashType = Self::HashType>,
        rpc: &dyn TransactionsSink<Self::HashType>,
    ) -> ExecutionResult<Self::HashType>
    where
        Self: Sized + 'static,
    {
        if self.is_watched() {
            self.send_watched_tx(log, rpc).await
        } else {
            self.send_tx(log, rpc).await
        }
    }
}

trait TxTaskStore {
    type HashType: BlockHash;
    fn pop() -> Option<Box<dyn TxTask<HashType = Self::HashType>>>;
}

struct FakeTxTask {
    tx: FakeTransaction,
    watched: bool,
}

impl FakeTxTask {
    fn new_watched(tx: FakeTransaction) -> Self {
        Self { tx, watched: true }
    }
    fn new_unwatched(tx: FakeTransaction) -> Self {
        Self { tx, watched: false }
    }
}

#[async_trait]
impl TxTask for FakeTxTask {
    type HashType = FakeHash;
    fn tx(&self) -> &dyn Transaction<HashType = FakeHash> {
        &self.tx
    }

    fn is_watched(&self) -> bool {
        self.watched
    }
}

struct Runner<H: BlockHash, T: TxTask<HashType = H>, Sink: TransactionsSink<H>> {
    logs: HashMap<H, DefaultExecutionLog<H>>,
    transactions: Vec<T>,
    rpc: Sink,
}

type FakeTxRunner = Runner<FakeHash, FakeTxTask, FakeTransactionSink>;

impl<H: BlockHash, T: TxTask<HashType = H> + Send + 'static, Sink: TransactionsSink<H>>
    Runner<H, T, Sink>
{
    fn new(rpc: Sink, transactions: Vec<T>) -> Self {
        let logs = transactions
            .iter()
            .map(|t| (t.tx().hash(), DefaultExecutionLog::default()))
            .collect();

        Self {
            logs,
            transactions,
            rpc,
        }
    }

    pub async fn run_poc2(&mut self) {
        let mut workers = FuturesUnordered::new();

        let mut i = 0;
        for _ in 0..15000 {
            if let Some(t) = self.transactions.pop() {
                let t = Box::new(t);
                let log = &self.logs[&t.tx().hash()];
                log.push_event(ExecutionEvent::popped());
                workers.push(t.execute(log, &self.rpc));
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
                            if let Some(task) = self.transactions.pop() {
                                let task = Box::from(task);
                                let log = &self.logs[&task.tx().hash()];
                                log.push_event(ExecutionEvent::popped());
                                workers.push(task.execute(log, &self.rpc));
                            }
                            info!(?result, workers_len=workers.len(), "FINISHED");
                        }
                        None => {
                            info!("done");
                            break;
                        }
                    }
                }
            }
        }
        info!("logs {:#?}", self.logs);
    }
}

mod fuck {
    use super::subxt_transaction::EthRuntimeConfig;
    use std::error::Error;
    use std::sync::Arc;
    use std::time::Duration;
    use subxt::backend::legacy::LegacyBackend;
    use subxt::OnlineClient;
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
    pub async fn connect(url: &str) -> Result<OnlineClient<EthRuntimeConfig>, Box<dyn Error>> {
        for i in 0..MAX_ATTEMPTS {
            info!("Attempt #{}: Connecting to {}", i, url);
            // let maybe_client = OnlineClient::<EthRuntimeConfig>::from_url(url).await;
            let backend = LegacyBackend::builder().build(subxt::backend::rpc::RpcClient::new(
                my_jsonrpsee_helpers::client(url).await?,
            ));
            let maybe_client = OnlineClient::from_backend(Arc::new(backend)).await;

            // let maybe_client = OnlineClient::<EthRuntimeConfig>::from_rpc_client(client);
            match maybe_client {
                Ok(client) => {
                    info!("Connection established to: {}", url);
                    return Ok(client);
                }
                Err(err) => {
                    info!("API client {} error: {:?}", url, err);
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            };
        }

        let err = format!(
            "Failed to connect to {} after {} attempts",
            url, MAX_ATTEMPTS
        );
        info!("{}", err);
        Err(err.into())
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
    #[tokio::test]
    async fn oh_god() {
        init_logger();

        let rpc = FakeTransactionSink::new();
        let mut transactions = (0..10)
            .map(|i| FakeTxTask::new_watched(FakeTransaction::new_finalizable_quick(i)))
            // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
            .collect::<Vec<_>>();

        transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_invalid(
            11u32, 300,
        )));
        transactions.push(FakeTxTask::new_unwatched(FakeTransaction::new_error(
            12u32, 300,
        )));

        let mut r = Runner::new(rpc, transactions);
        r.run_poc2().await;
    }

    struct SubxtEthTxTask {
        tx: TransactionEth,
        watched: bool,
    }

    impl SubxtEthTxTask {
        fn new_watched(tx: TransactionEth) -> Self {
            Self { tx, watched: true }
        }
        fn new_unwatched(tx: TransactionEth) -> Self {
            Self { tx, watched: false }
        }
    }

    #[async_trait]
    impl TxTask for SubxtEthTxTask {
        type HashType = <EthRuntimeConfig as subxt::Config>::Hash;
        fn tx(&self) -> &dyn Transaction<HashType = Self::HashType> {
            &self.tx
        }

        fn is_watched(&self) -> bool {
            self.watched
        }
    }

    fn xxx(api: &OnlineClient<EthRuntimeConfig>, nonce: u64) -> TransactionEth {
        let alith = dev::alith();
        let baltathar = dev::baltathar();

        let nonce = nonce;
        let tx_params = Params::new().nonce(nonce).build();

        // let tx_call = subxt::dynamic::tx("System", "remark", vec![Value::from_bytes("heeelooo")]);
        let tx_call = subxt::dynamic::tx(
            "Balances",
            "transfer_keep_alive",
            vec![
                // // Substrate:
                // Value::unnamed_variant("Id", [Value::from_bytes(receiver.public())]),
                // Eth:
                Value::unnamed_composite(vec![Value::from_bytes(alith.account_id())]),
                Value::u128(1u32.into()),
            ],
        );

        let tx: TransactionEth = api
            .tx()
            .create_signed_offline(&tx_call, &baltathar, tx_params)
            .unwrap();

        info!("tx hash: {:?}", tx.hash());

        tx
    }

    #[tokio::test]
    async fn oh_god2() {
        init_logger();

        // let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
        //     .await
        //     .unwrap();
        let api = fuck::connect("ws://127.0.0.1:9933").await.unwrap();

        let rpc = TransactionsSinkSubxt::<EthRuntimeConfig>::new();

        let mut transactions = (0..30000)
            .map(|i| SubxtEthTxTask::new_watched(xxx(&api, i + 30000)))
            .rev()
            // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
            .collect::<Vec<_>>();

        let mut r = Runner::new(rpc, transactions);
        r.run_poc2().await;
    }
}
