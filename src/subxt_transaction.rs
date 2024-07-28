use super::*;
use std::{any::Any, marker::PhantomData, pin::Pin};

use async_trait::async_trait;
use futures::FutureExt;
use futures::StreamExt;
use subxt::config::substrate::SubstrateExtrinsicParamsBuilder as Params;
use subxt::{
    dynamic::Value,
    tx::{SubmittableExtrinsic, TxStatus},
    OnlineClient, PolkadotConfig,
};
use subxt_core::config::BlockHash;
use subxt_signer::eth::{dev, AccountId20, Keypair as EthKeypair, Signature};
use tracing::info;

enum EthRuntimeConfig {}
impl subxt::Config for EthRuntimeConfig {
    type Hash = subxt::utils::H256;
    type AccountId = AccountId20;
    type Address = AccountId20;
    type Signature = Signature;
    type Hasher = subxt::config::substrate::BlakeTwo256;
    type Header =
        subxt::config::substrate::SubstrateHeader<u32, subxt::config::substrate::BlakeTwo256>;
    type ExtrinsicParams = subxt::config::SubstrateExtrinsicParams<Self>;
    type AssetId = u32;
}

type TransactionSubxt<C> = SubmittableExtrinsic<C, OnlineClient<C>>;

type TransactionSubstrate = TransactionSubxt<PolkadotConfig>;
type TransactionEth = TransactionSubxt<EthRuntimeConfig>;

impl<C: subxt::Config> Transaction for TransactionSubxt<C> {
    type HashType = <C as subxt::Config>::Hash;
    fn hash(&self) -> Self::HashType {
        self.hash()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

type StreamOf<I> = Pin<Box<dyn futures::Stream<Item = I> + Send>>;

struct TransactionsSinkSubxt<C: subxt::Config> {
    _p: PhantomData<C>,
}

#[async_trait]
impl<C: subxt::Config> TransactionsSink<<C as subxt::Config>::Hash> for TransactionsSinkSubxt<C> {
    async fn submit_and_watch(
        &self,
        tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
    ) -> Result<StreamOf<TransactionStatus<<C as subxt::Config>::Hash>>, Error> {
        let tx = tx.as_any().downcast_ref::<TransactionSubxt<C>>().unwrap();
        Ok(tx
            .submit_and_watch()
            .await
            .unwrap()
            .map(|e| {
                info!(evnt=?e, "TransactionsSinkSubxt::map");
                e.unwrap().into()
            })
            .boxed())
    }

    async fn submit(
        &self,
        tx: &dyn Transaction<HashType = <C as subxt::Config>::Hash>,
    ) -> Result<<C as subxt::Config>::Hash, Error> {
        Ok(tx.hash())
    }

    ///Current count of transactions being processed by sink
    fn count(&self) -> usize {
        todo!()
    }
}

#[tokio::test]
async fn test_subxt_send() -> Result<(), Box<dyn std::error::Error>> {
    init_logger();

    let api = OnlineClient::<EthRuntimeConfig>::from_insecure_url("ws://127.0.0.1:9933")
        .await
        .unwrap();

    let alith = dev::alith();
    let baltathar = dev::baltathar();

    let nonce = 11;
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

    let tx: TransactionSubxt<EthRuntimeConfig> = api
        .tx()
        .create_signed_offline(&tx_call, &baltathar, tx_params)
        .unwrap();

    info!("tx hash: {:?}", tx.hash());

    let sink = TransactionsSinkSubxt::<EthRuntimeConfig> {
        _p: Default::default(),
    };

    let tx: Box<dyn Transaction<HashType = <EthRuntimeConfig as subxt::Config>::Hash>> =
        Box::from(tx);
    let mut s = sink
        .submit_and_watch(&*tx)
        .await
        .unwrap()
        .map(|e| info!("event: {:?}", e));
    while let Some(_) = s.next().await {}
    Ok(())
}
