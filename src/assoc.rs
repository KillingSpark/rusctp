pub mod rx;
pub use rx::*;

pub mod tx;
pub use tx::*;

mod init;

use crate::{AssocId, TransportAddress};

pub struct Association {
    id: AssocId,
    rx: AssociationRx,
    tx: AssociationTx,
}

impl Association {
    pub(crate) fn new(
        id: AssocId,
        primary_path: TransportAddress,
        peer_verification_tag: u32,
        local_port: u16,
        peer_port: u16,
        init_local_tsn: u32,
        init_peer_tsn: u32,
        num_in_streams: u16,
        num_out_streams: u16,
    ) -> Self {
        Self {
            id,
            rx: AssociationRx::new(id, init_peer_tsn, num_in_streams),
            tx: AssociationTx::new(
                id,
                primary_path,
                peer_verification_tag,
                local_port,
                peer_port,
                init_local_tsn,
                num_out_streams,
            ),
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
