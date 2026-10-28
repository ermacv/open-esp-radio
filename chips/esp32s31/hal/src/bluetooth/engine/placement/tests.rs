use super::HostDma;
use crate::bluetooth::engine::*;

/// One application-selected arena; tuple splitting preserves placement.
type BluetoothStatic = (
    (
        BlePhyEngineStorage,
        (DirectionFindingWorkspaceStorage, LeDeviceTableStorage),
    ),
    (
        (
            SchedulerRolePoolStorage<DtmStorage, 1>,
            SchedulerRolePoolStorage<LegacyAdvertisingStorage, 1>,
        ),
        (
            (
                SchedulerRolePoolStorage<LegacyConnectableAdvertisingStorage, 1>,
                SchedulerRolePoolStorage<LegacyScanStorage, 1>,
            ),
            (
                SchedulerRolePoolStorage<PeripheralConnectionStorage, 1>,
                (LeRxChainStorage<2>, LeRxChainStorage<2>),
            ),
        ),
    ),
);

#[test]
#[allow(
    unsafe_code,
    reason = "the HAL DMA-static macro proves this test static placement"
)]
fn one_bluetooth_static_claims_and_initializes_every_owner_from_zero() {
    crate::dma_static! {
        static BLUETOOTH: BluetoothStatic = zeroed in ".dma.bss.bluetooth";
    }
    let (globals, roles) = BLUETOOTH.take().expect("one Bluetooth arena").split();
    assert!(BLUETOOTH.take().is_none());
    let (phy, rest) = globals.split();
    let (df, devices) = rest.split();
    let (transmitters, rest) = roles.split();
    let (dtm, advertising) = transmitters.split();
    let (advertisers_and_scanners, rest) = rest.split();
    let (connectable, scanning) = advertisers_and_scanners.split();
    let (connections, receivers) = rest.split();
    let (scan_rx, other_rx) = receivers.split();

    let phy = BlePhyEngineStorage::place::<HostDma<0x2f00_0100>>(phy);
    assert_eq!(
        phy.le_1m_packet_start_calibration(),
        BlePhyLe1MPacketStartCalibration::le_1m()
    );
    let df = DirectionFindingWorkspaceStorage::place::<HostDma<0x2f01_0000>>(df)
        .expect("workspace projection");
    assert!(df.is_disabled_baseline_initialized());
    let devices = LeDeviceTable::place::<HostDma<0x2f01_1000>>(devices);
    assert!(devices.is_empty());
    assert_eq!(devices.publication().count(), 0);

    let config = SchedulerAllocationConfig::new(2, 1, 0).expect("application capacities fit");
    let mut dtm = DtmPool::place::<HostDma<0x2f02_0000>>(dtm, config.direct_test_mode())
        .expect("DTM numbers fit");
    let mut advertising = LegacyAdvertisingPool::place::<HostDma<0x2f02_1000>>(
        advertising,
        config.advertising(0, 1).expect("advertising number"),
    )
    .expect("advertising numbers fit");
    let mut connectable = LegacyConnectableAdvertisingPool::place::<HostDma<0x2f02_2000>>(
        connectable,
        config.advertising(1, 1).expect("connectable number"),
    )
    .expect("connectable numbers fit");
    let mut scanning = LegacyScanPool::place::<HostDma<0x2f02_3000>>(scanning, config.scanning())
        .expect("scanning numbers fit");
    let mut connections =
        PeripheralConnectionPool::place::<HostDma<0x2f02_4000>>(connections, config.connections())
            .expect("connection numbers fit");
    for available in [
        dtm.acquire(),
        advertising.acquire(),
        connectable.acquire(),
        scanning.acquire(),
        connections.acquire(),
    ] {
        assert!(
            available.is_some(),
            "each zeroed role becomes a usable instance"
        );
    }
    let scan_rx = LeRxChain::place::<HostDma<0x2f03_0000>>(scan_rx, RxMemoryListClass::Scanning)
        .expect("two RX nodes");
    let other_rx =
        LeRxChain::place::<HostDma<0x2f03_1000>>(other_rx, RxMemoryListClass::NonScanning)
            .expect("two RX nodes");
    assert_ne!(scan_rx.initial_cursor(), other_rx.initial_cursor());
    let _ = (scan_rx.publication_head(), other_rx.publication_head());
}
