pub mod rx;
pub use rx::*;

pub mod tx;
pub use tx::*;

pub(crate) mod init;
mod srtt;

use crate::{packet::Tsn, AssocId, FakeAddr};

pub struct Association<FakeContent: FakeAddr> {
    id: AssocId,
    rx: AssociationRx<FakeContent>,
    tx: AssociationTx<FakeContent>,
}

impl<FakeContent: FakeAddr> Association<FakeContent> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: AssocId,
        init_peer_tsn: Tsn,
        num_in_streams: u16,
        in_buffer_limit: usize,
        tx_settings: AssocTxSettings<FakeContent>,
    ) -> Self {
        Self {
            id,
            rx: AssociationRx::new(id, init_peer_tsn, num_in_streams, in_buffer_limit),
            tx: AssociationTx::new(id, tx_settings),
        }
    }

    pub fn id(&self) -> AssocId {
        self.id
    }

    pub fn split(self) -> (AssociationRx<FakeContent>, AssociationTx<FakeContent>) {
        (self.rx, self.tx)
    }

    pub fn split_mut(
        &mut self,
    ) -> (
        &mut AssociationRx<FakeContent>,
        &mut AssociationTx<FakeContent>,
    ) {
        (&mut self.rx, &mut self.tx)
    }

    pub fn tx(&self) -> &AssociationTx<FakeContent> {
        &self.tx
    }

    pub fn tx_mut(&mut self) -> &mut AssociationTx<FakeContent> {
        &mut self.tx
    }

    pub fn rx(&self) -> &AssociationRx<FakeContent> {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut AssociationRx<FakeContent> {
        &mut self.rx
    }
}

pub enum ShutdownState {
    TryingTo,
    ShutdownSent,
    ShutdownReceived,
    ShutdownAckSent,
    Complete,
    AbortReceived,
}

impl ShutdownState {
    pub fn is_completely_shutdown(this: Option<&Self>) -> bool {
        matches!(
            this,
            Some(ShutdownState::Complete) | Some(ShutdownState::AbortReceived)
        )
    }
}
