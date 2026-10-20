use super::*;
use crate::ll::model::{ModelAddress, addresses};

#[test]
fn each_typed_identity_selects_only_its_frame_and_unknown_input_is_rejected() {
    let addresses = addresses();
    let mut buffers = Ieee802154EngineBuffers::new();
    let identities = addresses
        .rx
        .iter()
        .chain([&addresses.tx, &addresses.enhanced_ack]);
    for (slot, &address) in identities.clone().enumerate() {
        addresses
            .frame_at_mut(&mut buffers, address)
            .unwrap()
            .write(&[slot as u8 + 1]);
    }
    for (slot, &address) in identities.enumerate() {
        assert_eq!(
            addresses.frame_at(&buffers, address).unwrap().read()[0],
            slot as u8 + 1
        );
    }
    assert!(
        addresses
            .frame_at(&buffers, ModelAddress::Unknown)
            .is_none()
    );
    assert!(
        addresses
            .frame_at_mut(&mut buffers, ModelAddress::Unknown)
            .is_none()
    );
}
