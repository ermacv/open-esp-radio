//! HIL-only failure of an external application service, not a radio fault hook.
//! The retained RAM backend is never changed by an injected load failure.
use super::state::State;
use core::convert::Infallible;
use oer_bluetooth_gatt_trouble::security::bonds::{BondStore, RamBondStore, StoreError};
use trouble_host::{BondInformation, Identity};

#[derive(Debug, Eq, PartialEq)]
pub struct InjectedBondLoadFailure;

pub struct Store<'a> {
    pub ram: &'a mut RamBondStore<1>,
    pub state: &'a State,
}

fn widen(error: StoreError<Infallible>) -> StoreError<InjectedBondLoadFailure> {
    match error {
        StoreError::Full => StoreError::Full,
        StoreError::AlreadyStored => StoreError::AlreadyStored,
        StoreError::InsufficientSecurity => StoreError::InsufficientSecurity,
        StoreError::InvalidSlot => StoreError::InvalidSlot,
        StoreError::Backend(never) => match never {},
    }
}

impl BondStore for Store<'_> {
    type Error = InjectedBondLoadFailure;

    fn capacity(&self) -> usize {
        self.ram.capacity()
    }

    async fn load(
        &mut self,
        slot: usize,
    ) -> Result<Option<BondInformation>, StoreError<Self::Error>> {
        if self.state.take_bond_load_fault() {
            return Err(StoreError::Backend(InjectedBondLoadFailure));
        }
        self.ram.load(slot).await.map_err(widen)
    }

    async fn insert(&mut self, bond: BondInformation) -> Result<(), StoreError<Self::Error>> {
        self.ram.insert(bond).await.map_err(widen)
    }

    async fn remove(&mut self, identity: Identity) -> Result<bool, StoreError<Self::Error>> {
        self.ram.remove(identity).await.map_err(widen)
    }
}
