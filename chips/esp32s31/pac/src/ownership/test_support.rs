//! Host-test construction of register sets from validation partitions.

use super::{Ieee802154InterruptSetup, Ieee802154TaskRegisters, RadioPartitions};

pub(crate) fn ieee802154_task() -> (Ieee802154TaskRegisters, Ieee802154InterruptSetup) {
    let RadioPartitions { ieee802154, .. } = RadioPartitions::for_host_test();
    Ieee802154TaskRegisters::new(ieee802154)
}
