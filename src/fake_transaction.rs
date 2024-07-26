use super::*;

#[derive(Clone)]
struct EventDef {
    event: TransactionStatus<FakeHash>,
    delay: u32,
}

impl EventDef {
    fn validated(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Validated,
            delay,
        }
    }
    fn broadcasted(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Broadcasted,
            delay,
        }
    }
    fn in_block(h: FakeHash, delay: u32) -> Self {
        Self {
            event: TransactionStatus::InBlock(h),
            delay,
        }
    }
    fn finalized(h: FakeHash, delay: u32) -> Self {
        Self {
            event: TransactionStatus::Finalized(h),
            delay,
        }
    }
    fn dropped(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Dropped,
            delay,
        }
    }
    fn invalid(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Invalid,
            delay,
        }
    }
    fn error(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Error,
            delay,
        }
    }
}

#[derive(Clone)]
struct EventsStreamDef(Vec<EventDef>);

pub(crate) struct FakeTransaction {
    hash: FakeHash,
    stream_def: EventsStreamDef,
}

impl Transaction<FakeHash> for FakeTransaction {
    fn hash(&self) -> FakeHash {
        self.hash
    }
}

impl FakeTransaction {
    pub(crate) fn new(hash: FakeHash, stream_def: EventsStreamDef) -> Self {
        Self { stream_def, hash }
    }

    pub(crate) fn new_finalizable(hash: FakeHash) -> Self {
        Self::new(
            hash,
            EventsStreamDef(vec![
                EventDef::broadcasted(100),
                EventDef::validated(300),
                EventDef::in_block(1u32, 1000),
                EventDef::in_block(2, 1000),
                EventDef::in_block(3, 1000),
                EventDef::finalized(2, 2000),
            ]),
        )
    }

    pub(crate) fn events(&self) -> StreamOf<TransactionStatus<FakeHash>> {
        let def = self.stream_def.clone();
        stream::unfold(def.0.into_iter(), move |mut i| async move {
            if let Some(EventDef { event, delay }) = i.next() {
                tokio::time::sleep(Duration::from_millis(delay.into())).await;
                info!("play: {event:?}");
                Some((event, i))
            } else {
                None
            }
        })
        .boxed()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn fake_transaction_events() {
        init_logger();
        let t = FakeTransaction::new(
            1,
            EventsStreamDef(vec![
                EventDef::broadcasted(100),
                EventDef::validated(300),
                EventDef::in_block(1u32, 1000),
                EventDef::in_block(2, 1000),
                EventDef::in_block(3, 1000),
                EventDef::finalized(2, 2000),
            ]),
        );
        let v = t.events().collect::<Vec<_>>().await;
        assert_eq!(
            v,
            vec![
                TransactionStatus::Broadcasted,
                TransactionStatus::Validated,
                TransactionStatus::InBlock(1u32),
                TransactionStatus::InBlock(2),
                TransactionStatus::InBlock(3),
                TransactionStatus::Finalized(2)
            ]
        );
        info!("{v:?}")
    }
}
