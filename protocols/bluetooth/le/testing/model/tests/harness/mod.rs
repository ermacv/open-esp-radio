//! What the model's tests share: an installation, the values a port's
//! calls take, and the radio's life around a run.

#![allow(dead_code)]

use core::task::{Context, Waker};

use oer_protocols_bluetooth_le_contracts_port::{
    AdvertisingData, AdvertisingEvent, AdvertisingEventKind, AdvertisingFilterPolicy,
    AdvertisingReception, AdvertisingSet, ChannelSpacing, ConnectionEvent, ConnectionEventKind,
    ConnectionEventTiming, ConnectionPriority, ConnectionSetup, Install, LeInstant, LePhy, LePort,
    LeWindow, PortPools, RadioDuration, RoleRights, ScanFilterPolicy, ScanType, ScanWindow,
    ScannerSetup, StartPoll, StopFrom, StopPoll, StopRight, Stopped, Supported, TestWindow,
    TxPower,
};
use oer_protocols_bluetooth_le_testing_model::{ModelCaps, ModelPort};
use oer_protocols_bluetooth_le_wire::{
    AccessAddress, AdvertisingChannel, AdvertisingChannels, AdvertisingPdu, CrcInit, DataChannel,
    LeDeviceAddress, LeDeviceAddressKind,
};
use oer_protocols_radio_port::{Mint, branded};

/// The model of the geometry `ADV, SCAN, CONN, EVENTS, TX, RX, TEST`.
pub type Model<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> = ModelPort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>;

/// Microseconds on the model's clock.
pub const fn at(micros: u64) -> LeInstant {
    LeInstant::from_micros(micros)
}

/// A context that wakes nothing: the model answers its polls at once.
pub fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// Run `body` with a fresh installation of the geometry: the model built
/// from its key at `at(0)`, the machine's rights beside it.
pub fn installed<
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>(
    body: impl for<'id> FnOnce(
        &mut Model<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>,
        Stopped<'id>,
        StopRight<'id>,
        RoleRights<'id, ADV, SCAN, CONN, TEST>,
    ),
) {
    let future = core::pin::pin!(branded(
        async |mint: Mint<'_, PortPools<ModelCaps, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>>| {
            let Install {
                key,
                stopped,
                right,
                roles,
            } = mint.into_pools();
            let mut port = ModelPort::new(key, at(0));
            body(&mut port, stopped, right, roles);
        }
    ));
    assert!(core::future::Future::poll(future, &mut cx()).is_ready());
}

/// Start the stopped radio and run `run`, then stop it, and answer the
/// radio stopped.
pub fn running<
    'id,
    T,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>(
    port: &mut Model<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>,
    stopped: Stopped<'id>,
    right: &mut StopRight<'id>,
    run: impl FnOnce(&mut Model<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>) -> T,
) -> (Stopped<'id>, T) {
    let starting = port.start(stopped);
    let StartPoll::Running(running) = port.poll_start(starting, &mut cx()) else {
        panic!("the model's start confirms");
    };
    let kept = run(port);
    let stopping = port.stop(right, StopFrom::Running(running));
    let StopPoll::Stopped(stopped) = port.poll_stop(stopping, &mut cx()) else {
        panic!("the model's stop ends at once");
    };
    (stopped, kept)
}

/// A power under the open test domain's ceiling.
pub fn power() -> TxPower {
    let band = oer_protocols_radio_regulatory::TEST_DOMAIN
        .permit(oer_protocols_bluetooth_le_contracts_port::LeBand)
        .unwrap();
    let ceiling = band.tx_ceiling(oer_protocols_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    TxPower::within(
        oer_protocols_bluetooth_le_contracts_port::LeTxCeiling::new(band, ceiling),
        0,
    )
    .unwrap()
}

/// The model's own address.
pub const OWN: LeDeviceAddress =
    LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Random);

/// A nonconnectable advertisement.
pub const ADV_NONCONN_IND: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];
/// A scannable advertisement.
pub const ADV_SCAN_IND: [u8; 8] = [0x06, 6, 1, 2, 3, 4, 5, 6];
/// Its scan response.
pub const SCAN_RSP: [u8; 8] = [0x04, 6, 1, 2, 3, 4, 5, 6];

/// The advertising channels 1.5 ms apart.
pub const SPACING: ChannelSpacing<ModelCaps> =
    ChannelSpacing::new(RadioDuration::from_micros(1_500)).unwrap();

/// A connection event's priority.
pub const PRIORITY: ConnectionPriority = ConnectionPriority::new(8).unwrap();

/// A nonconnectable advertising set.
pub fn nonconnectable() -> Supported<ModelCaps, AdvertisingSet<'static>> {
    Supported::new(AdvertisingSet {
        data: AdvertisingData {
            pdu: AdvertisingPdu::new(&ADV_NONCONN_IND).unwrap(),
            reception: AdvertisingReception::None,
        },
        own: OWN,
        peer: None,
        filter_policy: AdvertisingFilterPolicy::AcceptAll,
        tx_power: power(),
        phy: LePhy::Le1M,
    })
    .unwrap()
}

/// A scannable advertising set that answers with [`SCAN_RSP`].
pub fn scannable() -> Supported<ModelCaps, AdvertisingSet<'static>> {
    Supported::new(AdvertisingSet {
        data: AdvertisingData {
            pdu: AdvertisingPdu::new(&ADV_SCAN_IND).unwrap(),
            reception: AdvertisingReception::ScanResponse(AdvertisingPdu::new(&SCAN_RSP).unwrap()),
        },
        own: OWN,
        peer: None,
        filter_policy: AdvertisingFilterPolicy::AcceptAll,
        tx_power: power(),
        phy: LePhy::Le1M,
    })
    .unwrap()
}

/// An advertising event on the three channels at `anchor`, placed at `now`:
/// it runs 4.5 ms.
pub fn advertising_event(
    anchor: LeInstant,
    now: LeInstant,
) -> Supported<ModelCaps, AdvertisingEvent<ModelCaps>> {
    Supported::placed(
        AdvertisingEvent {
            anchor,
            channels: AdvertisingChannels::ALL,
            channel_spacing: SPACING,
            coexistence: AdvertisingEventKind::Ordinary,
        },
        now,
    )
    .unwrap()
}

/// A window of `length` microseconds at `start`.
pub fn window(start: LeInstant, length: u64) -> LeWindow {
    LeWindow::new(start, RadioDuration::from_micros(length)).unwrap()
}

/// An active scanner.
pub fn scanner() -> Supported<ModelCaps, ScannerSetup> {
    Supported::new(ScannerSetup {
        scan_type: ScanType::Active,
        filter_policy: ScanFilterPolicy::AcceptAll,
        own: OWN,
        tx_power: power(),
        phy: LePhy::Le1M,
    })
    .unwrap()
}

/// A scan window of 10 ms at `start`, placed at `now`.
pub fn scan_window(start: LeInstant, now: LeInstant) -> Supported<ModelCaps, ScanWindow> {
    Supported::placed(
        ScanWindow {
            channel: AdvertisingChannel::Channel37,
            window: window(start, 10_000),
        },
        now,
    )
    .unwrap()
}

/// A peripheral connection.
pub fn link() -> Supported<ModelCaps, ConnectionSetup> {
    Supported::new(ConnectionSetup {
        access_address: AccessAddress([1, 2, 3, 4]),
        crc_init: CrcInit([1, 2, 3]),
        created_at: at(0),
        tx_power: power(),
        phy: LePhy::Le1M,
    })
    .unwrap()
}

/// A connection event of 2 ms at `start`, placed at `now`.
pub fn connection_event(start: LeInstant, now: LeInstant) -> Supported<ModelCaps, ConnectionEvent> {
    Supported::placed(
        ConnectionEvent {
            channel: DataChannel::new(0).unwrap(),
            window: window(start, 2_000),
            interval: RadioDuration::from_micros(30_000),
            timing: ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(1_000),
                widening: RadioDuration::from_micros(16),
            },
            priority: PRIORITY,
            coexistence: ConnectionEventKind::Ordinary,
        },
        now,
    )
    .unwrap()
}

/// A test's window of 1 ms at `start`, placed at `now`.
pub fn test_window(start: LeInstant, now: LeInstant) -> Supported<ModelCaps, TestWindow> {
    Supported::placed(
        TestWindow {
            window: window(start, 1_000),
        },
        now,
    )
    .unwrap()
}
