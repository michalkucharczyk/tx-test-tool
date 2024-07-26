use futures::FutureExt;
use std::marker::PhantomData;

use self::fake_transaction::{FakeHash, FakeTransaction};
use self::fake_transaction_sink::FakeTransactionSink;

use super::*;

pub async fn send_txs_poc(i: usize) -> (usize, u64, Instant) {
    let delay = rand::thread_rng().gen_range(1000..10000);
    info!(delay = delay, i = i, "send_txs start");
    let start = Instant::now();
    tokio::time::sleep(Duration::from_millis(delay)).await;
    info!(delay = delay, i = i, "send_txs done");
    (i, delay, start)
}

pub async fn run_poc() {
    let mut workers = FuturesUnordered::new();
    let mut cnt = 0;

    for i in 0..10 {
        workers.push(send_txs_poc(i));
        cnt = i;
    }

    loop {
        select! {
            done = workers.next() => {
                match done {
                    Some((i, delay, started)) => {
                        cnt = cnt + 1;
                        if cnt > 50 {
                            break;
                        }
                        let elapsed = started.elapsed();
                        let diff = elapsed.as_millis() - delay as u128;
                        info!(%i, %delay, actual=?elapsed, %diff, %cnt, workers_len=workers.len(), "FINISHED i:{}", i);
                        workers.push(send_txs_poc(cnt));
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

struct Runner<H: BlockHash> {
    _data: PhantomData<H>,
}

impl<H: BlockHash> Runner<H> {
    fn new() -> Self {
        Self {
            _data: Default::default(),
        }
    }
}

impl<H: BlockHash> Runner<H> {
    pub async fn send_txs(
        &self,
        tx: Box<dyn Transaction<HashType = H>>,
        rpc: &dyn TransactionsSink<H>,
    ) -> Result<H, ()> {
        let hash = tx.hash();
        let result = rpc.submit_and_watch(&*tx).await;
        match result {
            Ok(mut stream) => {
                while let Some(status) = stream.next().await {
                    match status {
                        TransactionStatus::Validated => {
                            info!("VALIDATED");
                        }
                        _ => {}
                    }
                }
                Ok(hash)
            }
            Err(_) => panic!(),
        }
    }
}

pub async fn run_poc2() {
    let runner = Runner::<FakeHash>::new();
    let rpc = FakeTransactionSink::new();

    let mut transactions = (0..10_000)
        .map(FakeTransaction::new_finalizable_quick)
        .map(|t| Box::from(t) as Box<dyn Transaction<HashType = FakeHash>>)
        .collect::<Vec<_>>();

    let mut workers = FuturesUnordered::new();

    for _ in 0..5000 {
        let tx = transactions.pop().unwrap();
        workers.push(runner.send_txs(tx, &rpc));
    }

    loop {
        select! {
            done = workers.next() => {
                // info!(?done, workers_len=workers.len(), "DONE");
                match done {
                    Some(tx_hash) => {
                        if let Some(tx) = transactions.pop() {
                            // let elapsed = started.elapsed();
                            workers.push(runner.send_txs(tx, &rpc));
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
}

//////

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn oh_god() {
        init_logger();
        run_poc2().await;
    }
}
