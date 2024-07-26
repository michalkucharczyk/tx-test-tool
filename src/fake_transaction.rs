use super::*;

pub type FakeHash = u32;
impl Hash for FakeHash {}

#[derive(Clone)]
pub struct EventDef {
    event: TransactionStatus<FakeHash>,
    delay: u32,
}

impl EventDef {
    pub fn validated(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Validated,
            delay,
        }
    }
    pub fn broadcasted(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Broadcasted,
            delay,
        }
    }
    pub fn in_block(h: FakeHash, delay: u32) -> Self {
        Self {
            event: TransactionStatus::InBlock(h),
            delay,
        }
    }
    pub fn finalized(h: FakeHash, delay: u32) -> Self {
        Self {
            event: TransactionStatus::Finalized(h),
            delay,
        }
    }
    pub fn dropped(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Dropped,
            delay,
        }
    }
    pub fn invalid(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Invalid,
            delay,
        }
    }
    pub fn error(delay: u32) -> Self {
        Self {
            event: TransactionStatus::Error,
            delay,
        }
    }
}

#[derive(Clone)]
pub struct EventsStreamDef(Vec<EventDef>);

impl From<Vec<EventDef>> for EventsStreamDef {
    fn from(value: Vec<EventDef>) -> Self {
        Self(value)
    }
}

pub struct FakeTransaction {
    hash: FakeHash,
    stream_def: EventsStreamDef,
}

impl Transaction<FakeHash> for FakeTransaction {
    fn hash(&self) -> FakeHash {
        self.hash
    }
}

impl FakeTransaction {
    pub fn new(hash: FakeHash, stream_def: EventsStreamDef) -> Self {
        Self { stream_def, hash }
    }

    pub fn new_droppable(hash: FakeHash, delay: u32) -> Self {
        Self::new(hash, EventsStreamDef(vec![EventDef::dropped(delay)]))
    }

    pub fn new_invalid(hash: FakeHash, delay: u32) -> Self {
        Self::new(hash, EventsStreamDef(vec![EventDef::invalid(delay)]))
    }

    pub fn new_error(hash: FakeHash, delay: u32) -> Self {
        Self::new(hash, EventsStreamDef(vec![EventDef::error(delay)]))
    }

    pub fn new_finalizable_quick(hash: FakeHash) -> Self {
        Self::new(
            hash,
            EventsStreamDef(vec![
                EventDef::broadcasted(10),
                EventDef::validated(10),
                EventDef::in_block(1u32, 10),
                EventDef::in_block(2, 10),
                EventDef::in_block(3, 10),
                EventDef::finalized(2, 10),
            ]),
        )
    }

    pub fn new_finalizable(hash: FakeHash) -> Self {
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

    pub fn events(&self) -> StreamOf<TransactionStatus<FakeHash>> {
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

    pub async fn submit_result(&self) -> Result<FakeHash, Box<dyn std::error::Error>> {
        let EventDef { event, delay } = self
            .stream_def
            .clone()
            .0
            .pop()
            .expect("there shall be at least event. qed.");
        tokio::time::sleep(Duration::from_millis(delay.into())).await;
        info!("submit_result: delayed: {:?}", self.hash);
        match event {
            TransactionStatus::Finalized(_) => Ok(self.hash),
            TransactionStatus::Dropped => {
                Err(Box::from(Error::Other("submit-error:dropped".to_string())))
            }
            TransactionStatus::Invalid => {
                Err(Box::from(Error::Other("submit-error:invalid".to_string())))
            }
            TransactionStatus::Error => {
                Err(Box::from(Error::Other("submit-error:error".to_string())))
            }
            TransactionStatus::Validated
            | TransactionStatus::Broadcasted
            | TransactionStatus::InBlock(_) => todo!(),
        }
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
