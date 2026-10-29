//! The eFuse identity over host memory.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::instances;

use super::{EfuseRegisters, FactoryIdentity, Interface};

/// Host memory of `bytes` bytes, each word `fill`.
fn memory(bytes: usize, fill: u32) -> Vec<u32> {
    vec![fill; bytes.div_ceil(4)]
}

#[test]
fn the_identity_derives_each_interface_address_from_the_factory_mac() {
    let mut words = memory(instances::efuse::SIZE, 0x0011_2033);
    let identity = FactoryIdentity::read(
        EfuseRegisters::host(&mut words).expect("host memory holds the block"),
    );
    let base = identity.base_mac();
    assert_eq!(base, [0x20, 0x33, 0x00, 0x11, 0x20, 0x33]);
    assert_eq!(identity.mac_extension(), 0x0011);
    assert_eq!(identity.address(Interface::Station), base);
    assert_eq!(
        identity.address(Interface::AccessPoint),
        [0x22, 0x33, 0x00, 0x11, 0x20, 0x33]
    );
}
