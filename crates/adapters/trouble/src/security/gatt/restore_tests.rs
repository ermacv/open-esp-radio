//! Exercise the real Host working set and application restoration boundary.

use super::*;
use bt_hci::controller::ExternalController;
use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_hci::{BluetoothPublicDeviceAddress, LeControllerBootstrapConfig};
use oer_bluetooth_hci_transport::LeControllerHciResources;

fn ready<T>(future: impl Future<Output = T>) -> T {
    match pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("unexpected wait in in-memory restoration"),
    }
}

fn bond(peer: u8) -> BondInformation {
    BondInformation::new(
        Identity::from(Address::random([peer, 2, 3, 4, 5, 0xc6])),
        LongTermKey::new(u128::from(peer)),
        SecurityLevel::EncryptedAuthenticated,
        true,
    )
}

// Deliberately allows corrupt imports and read errors that RamBondStore rejects
// at insertion. The application must validate an arbitrary storage backend.
struct Import {
    slots: [Option<BondInformation>; 3],
    fail: Option<usize>,
}
impl BondStore for Import {
    type Error = ();
    fn capacity(&self) -> usize {
        self.slots.len()
    }
    async fn load(&mut self, slot: usize) -> Result<Option<BondInformation>, StoreError<()>> {
        if self.fail == Some(slot) {
            Err(StoreError::Backend(()))
        } else {
            Ok(self.slots[slot].clone())
        }
    }
    async fn insert(&mut self, _: BondInformation) -> Result<(), StoreError<()>> {
        panic!("restoration must not enroll")
    }
    async fn remove(&mut self, _: Identity) -> Result<bool, StoreError<()>> {
        panic!("restoration must not mutate the store")
    }
}

fn transport() -> LeControllerHciResources<NoopRawMutex, 4, 4, 258> {
    LeControllerHciResources::new(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            251,
            4,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn restoration_enumerates_holes_into_real_host_without_owning_store() {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut resources = HostResources::<DefaultPacketPool, 1, 3, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 4>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let mut store = Import {
        slots: [Some(bond(1)), None, Some(bond(2))],
        fail: None,
    };
    assert_eq!(ready(restore(&stack, &mut store)).unwrap(), 2);
    assert!(stack.with_bond_information(|b| b.len() == 2 && b[0] == bond(1) && b[1] == bond(2)));
    stack.remove_bond_information(bond(1).identity).unwrap();
    assert!(ready(store.load(0)).unwrap().is_some());
    assert!(ready(store.load(1)).unwrap().is_none());
}

#[test]
fn incomplete_import_stops_application_before_advertising_and_cannot_reuse_host() {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut resources = HostResources::<DefaultPacketPool, 1, 3, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 4>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let comparison = NumericComparison::new();
    let mut store = Import {
        slots: [Some(bond(1)), None, None],
        fail: Some(1),
    };
    let result = ready(run(&stack, &mut store, &comparison, |_| {
        panic!("no partial startup")
    }));
    assert!(matches!(
        result,
        Err(RunError::Store(StoreError::Backend(())))
    ));
    assert!(stack.with_bond_information(|b| b.len() == 1));
    store.fail = None;
    assert!(matches!(
        ready(run(&stack, &mut store, &comparison, |_| panic!("no reuse"))),
        Err(RunError::HostNotEmpty)
    ));
}

#[test]
fn duplicate_or_unauthenticated_import_never_reaches_advertising() {
    for duplicate in [false, true] {
        let mut transport = transport();
        let endpoints = transport.split();
        let mut resources = HostResources::<DefaultPacketPool, 1, 3, 1, 3>::new();
        let stack = trouble_host::new(
            ExternalController::<_, 4>::new(endpoints.host),
            &mut resources,
        )
        .build();
        let comparison = NumericComparison::new();
        let mut second = bond(if duplicate { 1 } else { 2 });
        if !duplicate {
            second.security_level = SecurityLevel::Encrypted;
        }
        let mut store = Import {
            slots: [Some(bond(1)), None, Some(second)],
            fail: None,
        };
        assert!(matches!(
            ready(run(&stack, &mut store, &comparison, |_| panic!(
                "no corrupt startup"
            ))),
            Err(RunError::InvalidBond)
        ));
    }
}
