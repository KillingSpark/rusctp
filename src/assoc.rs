pub mod rx;
pub use rx::*;

pub mod tx;
pub use tx::*;

mod init;

use crate::{AssocId, TransportAddress};

pub struct Association {
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
    ) -> Self {
        Self {
            rx: AssociationRx::new(id),
            tx: AssociationTx::new(
                id,
                primary_path,
                peer_verification_tag,
                local_port,
                peer_port,
            ),
        }
    }

    pub fn split(self) -> (AssociationRx, AssociationTx) {
        (self.rx, self.tx)
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
