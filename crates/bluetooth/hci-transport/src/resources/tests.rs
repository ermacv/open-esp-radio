use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use super::{LeControllerHciResources, LeControllerHciResourcesError};
use oer_bluetooth_hci::{BluetoothPublicDeviceAddress, LeControllerBootstrapConfig};

fn config(payload: u16, credits: u8) -> LeControllerBootstrapConfig {
    LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]),
        payload,
        credits,
    )
    .expect("nonzero test profile")
}

#[test]
fn advertised_acl_profile_must_fit_owned_storage_and_credits() {
    assert!(matches!(
        LeControllerHciResources::<NoopRawMutex, 2, 1, 30>::new(config(27, 1)),
        Err(LeControllerHciResourcesError::PacketCapacityTooSmall {
            required: 70,
            available: 30,
        })
    ));
    assert!(matches!(
        LeControllerHciResources::<NoopRawMutex, 1, 1, 80>::new(config(27, 2)),
        Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
            credits: 2,
            slots: 1,
        })
    ));
    let resources = LeControllerHciResources::<NoopRawMutex, 2, 2, 80>::new(config(27, 1)).unwrap();
    assert!(resources.is_pristine());
}
