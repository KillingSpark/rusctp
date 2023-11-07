pub mod rx;
pub use rx::*;

pub mod tx;
pub use tx::*;



use crate::{AssocId, TransportAddress};

pub struct Association {
    rx: AssociationRx,
    tx: AssociationTx,
}

impl Association {
    pub(crate) fn new(id: AssocId, primary_path: TransportAddress) -> Self {
        Self {
            rx: AssociationRx::new(id),
            tx: AssociationTx::new(id, primary_path),
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
