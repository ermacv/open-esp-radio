//! Host model memory for tests of this crate and its dependents.

use std::boxed::Box;

use oer_esp32s31_bluetooth_memory::{
    DirectionFindingWorkspaceModelAddress, DirectionFindingWorkspaceStorage, DtmPool, DtmStorage,
    LeDeviceTable, LeDeviceTableStorage, LeRxChain, LeRxChainModelAddress, LeRxChainStorage,
    LegacyAdvertisingPool, LegacyAdvertisingStorage, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingStorage, LegacyScanPool, LegacyScanStorage,
    PeripheralConnectionPool, PeripheralConnectionStorage, RxMemoryListClass,
    SchedulerAllocationConfig, SchedulerPoolModelAddress, SchedulerRolePoolStorage,
};

use crate::BluetoothRadioMemory;

/// Model memory: one instance of each role, two scanning and three
/// non-scanning receive packets.
pub type ModelMemory = BluetoothRadioMemory<1, 1, 1, 1, 2, 3>;

fn base(offset: u32) -> SchedulerPoolModelAddress {
    SchedulerPoolModelAddress::new(0x2f00_0000 + offset).unwrap()
}

fn chain<const N: usize>(class: RxMemoryListClass, offset: u32) -> LeRxChain<N> {
    LeRxChain::bind_model(
        Box::leak(Box::new(LeRxChainStorage::<N>::new())),
        class,
        LeRxChainModelAddress::new(0x2f00_0000 + offset).unwrap(),
    )
    .unwrap()
}

/// Leak fresh storage and bind every pool and chain at model addresses.
pub fn model_memory() -> ModelMemory {
    let numbers = SchedulerAllocationConfig::new(2, 1, 0).unwrap();
    BluetoothRadioMemory {
        legacy: LegacyAdvertisingPool::bind_model(
            Box::leak(Box::new(SchedulerRolePoolStorage::<
                LegacyAdvertisingStorage,
                1,
            >::new())),
            base(0x1000),
            numbers.advertising(0, 1).unwrap(),
        )
        .unwrap(),
        connectable: LegacyConnectableAdvertisingPool::bind_model(
            Box::leak(Box::new(SchedulerRolePoolStorage::<
                LegacyConnectableAdvertisingStorage,
                1,
            >::new())),
            base(0x2000),
            numbers.advertising(1, 1).unwrap(),
        )
        .unwrap(),
        scanners: LegacyScanPool::bind_model(
            Box::leak(Box::new(
                SchedulerRolePoolStorage::<LegacyScanStorage, 1>::new(),
            )),
            base(0x3000),
            numbers.scanning(),
        )
        .unwrap(),
        connections: PeripheralConnectionPool::bind_model(
            Box::leak(Box::new(SchedulerRolePoolStorage::<
                PeripheralConnectionStorage,
                1,
            >::new())),
            base(0x4000),
            numbers.connections(),
        )
        .unwrap(),
        dtm: DtmPool::bind_model(
            Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new())),
            base(0x6000),
            numbers.direct_test_mode(),
        )
        .unwrap(),
        scanning: chain(RxMemoryListClass::Scanning, 0x8000),
        non_scanning: chain(RxMemoryListClass::NonScanning, 0xa000),
        direction_finding: DirectionFindingWorkspaceStorage::pin_static_model(
            Box::leak(Box::new(DirectionFindingWorkspaceStorage::new())),
            DirectionFindingWorkspaceModelAddress::new(0x2f01_0000).unwrap(),
        )
        .unwrap()
        .binding()
        .link(),
        device_table: LeDeviceTable::bind_model(
            Box::leak(Box::new(LeDeviceTableStorage::new())),
            0x2f00_c000,
        )
        .unwrap(),
    }
}
