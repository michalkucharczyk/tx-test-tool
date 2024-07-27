use std::collections::HashMap;

use self::fake_transaction::{FakeHash, FakeTransaction};
use self::fake_transaction_sink::FakeTransactionSink;

use super::*;

#[derive(Debug)]
enum ExecutionEvent<H: BlockHash> {
    Popped(Instant),
    Sent(Instant),
    Resubmitted(Instant),
    SubmitResult(Instant, Result<(), String>),
    SubmitAndWatchResult(Instant, Result<(), String>),
    TxPoolEvent(Instant, TransactionStatus<H>),
}

impl<H: BlockHash> ExecutionEvent<H> {
    fn popped(t: Instant) -> Self {
        Self::Popped(t)
    }
    fn sent(t: Instant) -> Self {
        Self::Sent(t)
    }
    fn submit_and_watch_result(t: Instant, r: Result<(), String>) -> Self {
        Self::SubmitAndWatchResult(t, r)
    }
    fn submit_result(t: Instant, r: Result<(), String>) -> Self {
        Self::SubmitResult(t, r)
    }
}

impl<H: BlockHash> From<(Instant, TransactionStatus<H>)> for ExecutionEvent<H> {
    fn from(value: (Instant, TransactionStatus<H>)) -> Self {
        Self::TxPoolEvent(value.0, value.1)
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

#[derive(Default, Debug)]
struct DefaultExecutionLog<H: BlockHash> {
    events: RwLock<Vec<ExecutionEvent<H>>>,
}

impl<H: BlockHash> ExecutionLog for DefaultExecutionLog<H> {
    type HashType = H;

    fn push_event(&self, event: ExecutionEvent<Self::HashType>) {
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
enum ExecutionResult {
    NeedsResubmit,
    Done,
}

#[async_trait]
trait TxTask {
    type HashType: BlockHash;

    async fn send_tx(
        self,
        log: &dyn ExecutionLog<HashType = Self::HashType>,
        rpc: &dyn TransactionsSink<Self::HashType>,
    ) -> ExecutionResult;
}

trait TxTaskStore {
    type HashType: BlockHash;
    fn pop() -> Option<Box<dyn TxTask<HashType = Self::HashType>>>;
}

struct FakeTxTask {
    tx: FakeTransaction,
}

impl FakeTxTask {
    fn new(tx: FakeTransaction) -> Self {
        Self { tx }
    }

    fn tx(&self) -> &dyn Transaction<HashType = FakeHash> {
        &self.tx
    }
}

// #[async_trait]
// impl TxTask for FakeTxTask {
//     type HashType = FakeHash;
//
//     async fn send_tx(
//         self,
//         log: &dyn ExecutionLog<HashType = Self::HashType>,
//         rpc: &dyn TransactionsSink<Self::HashType>,
//     ) -> ExecutionResult {
//         self.send_tx_inner(log, rpc).await
//     }
// }

impl FakeTxTask {
    // type HashType = FakeHash;
    async fn send_tx_inner(
        self,
        log: &dyn ExecutionLog<HashType = FakeHash>,
        rpc: &dyn TransactionsSink<FakeHash>,
    ) -> ExecutionResult {
        log.push_event(ExecutionEvent::popped(Instant::now()));
        let submission = rpc.submit_and_watch(self.tx());
        log.push_event(ExecutionEvent::sent(Instant::now()));
        let result = submission.await;
        match result {
            Ok(mut stream) => {
                let mut result = None;
                log.push_event(ExecutionEvent::submit_and_watch_result(
                    Instant::now(),
                    Ok(()),
                ));
                while let Some(status) = stream.next().await {
                    result = if status.is_finalized() {
                        Some(ExecutionResult::Done)
                    } else if status.is_error() {
                        Some(ExecutionResult::NeedsResubmit)
                    } else {
                        None
                    };
                    log.push_event((Instant::now(), status).into())
                }
                result.expect("stream shall terminated with error or finalized. qed.")
            }
            Err(e) => {
                log.push_event(ExecutionEvent::submit_and_watch_result(
                    Instant::now(),
                    Err(e.to_string()),
                ));
                ExecutionResult::NeedsResubmit
            }
        }
    }
}

struct Runner {
    logs: HashMap<FakeHash, DefaultExecutionLog<FakeHash>>,
    transactions: Vec<FakeTxTask>,
    rpc: FakeTransactionSink,
}

impl Runner {
    fn new() -> Self {
        let transactions = (0..10_000)
            .map(|i| FakeTxTask::new(FakeTransaction::new_finalizable_quick(i)))
            // .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
            .collect::<Vec<_>>();

        let logs = transactions
            .iter()
            .map(|t| ((*t.tx()).hash(), DefaultExecutionLog::default()))
            .collect();

        let rpc = FakeTransactionSink::new();
        Self {
            logs,
            transactions,
            rpc,
        }
    }

    pub async fn run_poc2(&mut self) {
        let mut workers = FuturesUnordered::new();

        let mut i = 0;
        for _ in 0..5000 {
            let t = self.transactions.pop().unwrap();
            let log = &self.logs[&t.tx().hash()];
            workers.push(t.send_tx_inner(log, &self.rpc));
            i = i + 1;
        }

        loop {
            select! {
                done = workers.next() => {
                    // info!(?done, workers_len=workers.len(), "DONE");
                    match done {
                        Some(tx_hash) => {
                            if let Some(task) = self.transactions.pop() {
                                // let elapsed = started.elapsed();
                                let log = &self.logs[&task.tx().hash()];
                                workers.push(task.send_tx_inner(log, &self.rpc));
                            }
                            info!(?tx_hash, workers_len=workers.len(), "FINISHED");
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

//////

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oh_god() {
        init_logger();
        let mut r = Runner::new();
        r.run_poc2().await;
    }
}
