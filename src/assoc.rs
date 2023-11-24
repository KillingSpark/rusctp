pub mod rx;
pub use rx::*;

pub mod tx;
pub use tx::*;

mod init;
mod srtt;

use crate::{packet::Tsn, AssocId};

pub struct Association {
    id: AssocId,
    rx: AssociationRx,
    tx: AssociationTx,
}

impl Association {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: AssocId,
        init_peer_tsn: Tsn,
        num_in_streams: u16,
        in_buffer_limit: usize,
        tx_settings: AssocTxSettings,
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

    pub fn split(self) -> (AssociationRx, AssociationTx) {
        (self.rx, self.tx)
    }

    pub fn split_mut(&mut self) -> (&mut AssociationRx, &mut AssociationTx) {
        (&mut self.rx, &mut self.tx)
    }

    pub fn tx(&self) -> &AssociationTx {
        &self.tx
    }

    pub fn tx_mut(&mut self) -> &mut AssociationTx {
        &mut self.tx
    }

    pub fn rx(&self) -> &AssociationRx {
        &self.rx
    }

    pub fn rx_mut(&mut self) -> &mut AssociationRx {
        &mut self.rx
    }
}
