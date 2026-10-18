use core::{
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Waker},
};
use oer_time::{Clock as _, Timer as _};

use crate::datapath::{
    PinnedTxFrame, PinnedTxPool, PinnedTxResources,
    network::DatapathNetwork,
    owned::{DatapathTxConsumer, OwnedDatapathNetwork},
};

use oer_embassy_net_owned::{
    NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources, OwnedNetworkDevice,
};

use oer_esp32s31_hal::types::{MacHtAmpduCompletionObservation, MacTxCompletionObservation};

use oer_esp32s31_ieee80211_mac::{
    rx::HeGuardIntervalAndLtf,
    tx::{
        HeMcs, HeRate, HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate, TxSlot,
        ampdu::{HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage},
        protection::{BssProtection, HeTxopDurationRtsThreshold, TxProtection},
    },
};

use oer_esp32s31_ieee80211_sta::{
    connected_control::ConnectedControlTx,
    single_mpdu_tx::{ActionTxConfig, ConnectedTxSecurity},
};

use oer_ieee80211_mac::extensions::wmm::parse_wmm_parameter_element;

use super::*;

use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

mod terminal;

use super::test_support::*;

#[derive(Default)]
struct RecordingAggregateTxObserver {
    observations: std::sync::Mutex<std::vec::Vec<AggregateTxObservation>>,
    ordinary: std::sync::Mutex<
        std::vec::Vec<Option<oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxOutcome>>,
    >,
    terminal: std::sync::Mutex<std::vec::Vec<MacAmpduTxStatus<TxPhyRate>>>,
}

impl AggregateTxObserver for RecordingAggregateTxObserver {
    fn observe_station_ordinary(
        &self,
        outcome: Option<oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxOutcome>,
    ) {
        self.ordinary.lock().unwrap().push(outcome);
    }

    fn observe_station_terminal(&self, status: MacAmpduTxStatus<TxPhyRate>) {
        self.terminal.lock().unwrap().push(status);
    }

    fn now_micros(&self) -> u64 {
        0
    }

    fn observe(&self, observation: AggregateTxObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

impl RecordingAggregateTxObserver {
    fn observed(&self, expected: AggregateTxObservation) -> bool {
        self.observations.lock().unwrap().contains(&expected)
    }

    fn count(&self, expected: AggregateTxObservation) -> usize {
        self.observations
            .lock()
            .unwrap()
            .iter()
            .filter(|observation| **observation == expected)
            .count()
    }
}

const STANDARD_WMM: [u8; 26] = [
    221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42, 0x43,
    94, 0, 0x72, 0x32, 47, 0,
];
static BLOCK_ACK_STATUS: AtomicU8 = AtomicU8::new(0);

fn record_block_ack_status(tid: u8, operational: bool) {
    let bit = 1_u8 << tid;
    if operational {
        BLOCK_ACK_STATUS.fetch_or(bit, Ordering::Relaxed);
    } else {
        BLOCK_ACK_STATUS.fetch_and(!bit, Ordering::Relaxed);
    }
}

type OwnedNetwork<const F: usize, const H: usize, const T: usize, const Q: usize> =
    OwnedDatapathNetwork<'static, NoopRawMutex, F, H, T, Q, Q, Q>;

struct Device<const Q: usize = TEST_QUEUE_DEPTH> {
    inner: OwnedNetworkDevice<'static, NoopRawMutex, Q, Q>,
    allocator: PacketBufAllocator,
}

struct Network<
    const F: usize = TEST_FRAME_CAPACITY,
    const H: usize = TEST_HEADROOM,
    const T: usize = TEST_TRAILER,
    const Q: usize = TEST_QUEUE_DEPTH,
> {
    inner: OwnedNetwork<F, H, T, Q>,
}

struct TestTxToken<'a, const Q: usize> {
    device: &'a mut OwnedNetworkDevice<'static, NoopRawMutex, Q, Q>,
    packet: PacketBuf,
}

impl<const Q: usize> Device<Q> {
    fn transmit(&mut self, _context: &mut Context<'_>) -> Option<TestTxToken<'_, Q>> {
        let packet = self.allocator.try_alloc()?;
        Some(TestTxToken {
            device: &mut self.inner,
            packet,
        })
    }
}

impl<const Q: usize> TestTxToken<'_, Q> {
    fn consume<R>(mut self, length: usize, emit: impl FnOnce(&mut [u8]) -> R) -> R {
        assert!(length <= self.packet.capacity());
        self.packet.set_len(length);
        let result = emit(&mut self.packet);
        self.device
            .transmit(self.packet)
            .unwrap_or_else(|_| panic!("test owned TX queue has the advertised credit"));
        result
    }
}

impl<const F: usize, const H: usize, const T: usize, const Q: usize> Network<F, H, T, Q> {
    fn tx_consumer(&self) -> DatapathTxConsumer<'_, 'static, NoopRawMutex, F, H, T, Q> {
        self.inner.tx_consumer(NetworkInterfaceId::new(0))
    }

    fn try_receive_tx_direct(&self) -> Option<PinnedTxFrame<'static, NoopRawMutex, F, H, T, Q>> {
        self.tx_consumer().try_receive_direct()
    }

    fn tx_queue_len(&self) -> usize {
        self.inner.tx_queue_len(NetworkInterfaceId::new(0))
    }
}

fn context() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

fn send_frame(device: &mut Device, marker: u8) {
    device
        .transmit(&mut context())
        .expect("free pinned network slot")
        .consume(17, |frame| {
            frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
            frame[6..12].copy_from_slice(&STATION);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
            frame[14..].fill(marker);
        });
}

fn send_ipv4_dscp_frame(device: &mut Device, marker: u8, dscp: u8) {
    device
        .transmit(&mut context())
        .expect("free pinned network slot")
        .consume(18, |frame| {
            frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
            frame[6..12].copy_from_slice(&STATION);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
            frame[14] = 0x45;
            frame[15] = dscp << 2;
            frame[16..].fill(marker);
        });
}

fn send_short_frame(device: &mut Device) {
    device
        .transmit(&mut context())
        .expect("free pinned network slot")
        .consume(8, |frame| frame.fill(0));
}

fn make_network() -> (Device, Network) {
    make_network_config::<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, TEST_QUEUE_DEPTH>()
}

fn make_network_config<const F: usize, const H: usize, const T: usize, const Q: usize>()
-> (Device<Q>, Network<F, H, T, Q>) {
    let resources = std::boxed::Box::leak(std::boxed::Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        Q,
        Q,
    >::new()));
    let pool = PinnedTxPool::<F, H, T, Q>::pin_static(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedTxPool::new(),
    )));
    let tx_resources = std::boxed::Box::leak(std::boxed::Box::new(PinnedTxResources::<
        NoopRawMutex,
        F,
        H,
        T,
        Q,
    >::new()));
    let physical = tx_resources.split(pool);
    let packet_storage = std::boxed::Box::leak(std::boxed::Box::new(PacketPoolStorage::<Q>::new()));
    let packet_pool = std::boxed::Box::leak(std::boxed::Box::new(PacketPool::new(packet_storage)));
    let rx_storage = std::boxed::Box::leak(std::boxed::Box::new(PacketPoolStorage::<1>::new()));
    let rx_pool = std::boxed::Box::leak(std::boxed::Box::new(PacketPool::new(rx_storage)));
    let allocator = packet_pool.allocator();
    let (device, radio) = resources.split(NetworkInterfaceId::new(0), STATION, rx_pool.allocator());
    radio.link_controller().set_link_up(true);
    (
        Device {
            inner: device,
            allocator,
        },
        Network {
            inner: OwnedDatapathNetwork::new(radio, physical),
        },
    )
}

#[test]
fn idle_aggregate_returns_ordinary_and_storage_for_station_teardown() {
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let tx = ConnectedTx::<
        crate::datapath::PinnedTxFrame<
            '_,
            NoopRawMutex,
            TEST_FRAME_CAPACITY,
            TEST_HEADROOM,
            TEST_TRAILER,
            TEST_QUEUE_DEPTH,
        >,
        _,
        _,
        _,
        TEST_SLOTS,
        0,
        TEST_BUFFER_SIZE,
    >::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    assert!(
        core::mem::size_of_val(&tx) < 2_048,
        "connected TX must remain a movable handle over external retention arenas"
    );

    let returned = match tx.try_into_station_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("idle aggregate must decompose"),
    };
    assert_eq!(returned.aggregate.primary().state(), TxSlotState::Free);
    assert_eq!(returned.resources.slot.state(), TxSlotState::Free);
    assert_eq!(
        returned.sequences.peek_non_qos(),
        SequenceNumber::new(7).unwrap()
    );
    let ConnectedTxSecurity::Wpa2Personal(key) = returned.security else {
        panic!("WPA2 test owner must return its pairwise key");
    };
    key.clear(&mut hardware);
}

#[test]
fn idle_station_tx_lends_physical_owners_without_losing_role_state() {
    BLOCK_ACK_STATUS.store(0, Ordering::Relaxed);
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::<
        crate::datapath::PinnedTxFrame<
            '_,
            NoopRawMutex,
            TEST_FRAME_CAPACITY,
            TEST_HEADROOM,
            TEST_TRAILER,
            TEST_QUEUE_DEPTH,
        >,
        _,
        _,
        _,
        TEST_SLOTS,
        0,
        TEST_BUFFER_SIZE,
    >::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_block_ack_status_sink(record_block_ack_status);
    tx.set_block_ack_agreement(0, Some((3, true)));
    assert_eq!(BLOCK_ACK_STATUS.load(Ordering::Relaxed), 1);
    let block_ack_generation = tx.block_ack_generation(0);

    assert_eq!(
        ConnectedControlTx::start_action(
            &mut tx,
            &mut hardware,
            &[3, 0],
            ActionTxConfig::VENDOR_MANAGEMENT,
        ),
        Ok(DatapathControlProgress::TxPending),
    );
    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx());
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete),
    );

    let (ordinary, aggregate, parked) = tx
        .try_park()
        .unwrap_or_else(|_| panic!("idle station TX must lend both physical owners"));
    assert_eq!(ordinary.slot.state(), TxSlotState::Free);
    assert_eq!(aggregate.primary().state(), TxSlotState::Free);

    let mut tx = ConnectedTx::<
        crate::datapath::PinnedTxFrame<
            '_,
            NoopRawMutex,
            TEST_FRAME_CAPACITY,
            TEST_HEADROOM,
            TEST_TRAILER,
            TEST_QUEUE_DEPTH,
        >,
        _,
        _,
        _,
        TEST_SLOTS,
        0,
        TEST_BUFFER_SIZE,
    >::resume(ordinary, aggregate, parked);
    assert_eq!(tx.block_ack_window(0), Some(3));
    assert_eq!(tx.block_ack_generation(0), block_ack_generation);
    assert!(tx.block_ack_amsdu(0));
    assert!(matches!(
        tx.take_last_ordinary_outcome(),
        Some(SingleMpduTxOutcome::Success(_))
    ));

    let returned = tx
        .try_into_station_parts()
        .unwrap_or_else(|_| panic!("resumed station TX remains cleanly reclaimable"));
    assert_eq!(
        returned.sequences.peek_non_qos(),
        SequenceNumber::new(8).unwrap()
    );
    let ConnectedTxSecurity::Wpa2Personal(key) = returned.security else {
        panic!("WPA2 test owner must return its pairwise key");
    };
    key.clear(&mut hardware);
}

#[test]
fn first_frame_outside_fresh_aggregate_txop_falls_back_to_ordinary_tx() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::from_units_32_us(5).unwrap(),
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        observer.count(AggregateTxObservation::BlockAckOperational {
            tid: 0,
            operational: true,
        }),
        1,
    );

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    // The single MPDU leaves at the rate control's HE data schedule.
    assert_eq!(hardware.he_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert_eq!(hardware.ht_publications, 0);
    assert!(
        observer.observed(AggregateTxObservation::NetworkSingleMpdu {
            reason: NetworkSingleMpduReason::FreshAggregateCapacity,
            ethernet_length: 17,
        })
    );
    // Rate and publication budget come from the one schedule record the rate
    // control currently selects, not from the association's fixed attempt
    // limit that bounds management frames.
    let selection = tx.data_tx_selection();
    assert_eq!(selection.rate, tx.rate_control.tx_rate(tx.data_rate_policy));
    assert_eq!(
        selection.publication_limit,
        schedule_publication_limit(tx.rate_control.current_schedule())
    );
    hardware.ordinary_completion = Some(MacTxCompletionObservation::new_model(2, 0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE
            }
        ),
        Ok(WifiTxProgress::Pending),
    );
    assert!(observer.ordinary.lock().unwrap().is_empty());
    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx());
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let outcome = tx.ordinary.last_outcome().unwrap();
    assert_eq!(outcome.report().retries.cts_timeouts, 1);
    assert!(matches!(outcome, SingleMpduTxOutcome::Success(_)));
    assert_eq!(*observer.ordinary.lock().unwrap(), [Some(outcome)]);
}

#[test]
fn malformed_first_frame_reaches_ordinary_validation_before_aggregate_metadata() {
    let (mut device, network) = make_network();
    send_short_frame(&mut device);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    let sequence = tx.ordinary.peek_qos_sequence(0);

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Err(AggregateTxError::Ordinary(
            SingleMpduTxError::EthernetFrameTooShort
        ))
    );
    assert_eq!(tx.ordinary.peek_qos_sequence(0), sequence);
    assert_eq!(hardware.legacy_publications, 0);
    assert_eq!(hardware.he_publications, 0);
    assert_eq!(tx.aggregate_slot_state(), TxSlotState::Free);
}

#[test]
fn production_sized_he_frame_fits_a_fresh_default_txop_aggregate() {
    const FRAME_CAPACITY: usize = 1_600;
    const HEADROOM: usize = TEST_HEADROOM;
    const TRAILER: usize = 12;
    const QUEUE_DEPTH: usize = 3;
    let (mut device, network) =
        make_network_config::<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>();
    for marker in 1..=2 {
        device
            .transmit(&mut context())
            .expect("free production-sized pinned network slot")
            .consume(1_514, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<2_048>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert_eq!(hardware.he_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    }));
    #[cfg(feature = "tx-wait-probe")]
    {
        assert_eq!(tx.next_deadline(), Some(5_000));
        {
            let mut wait = core::pin::pin!(tx.wait_deadline());
            assert!(wait.as_mut().poll(&mut context()).is_ready());
        }
        assert_eq!(
            tx.service(&mut hardware, WifiTxWake::Deadline),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.next_deadline(), Some(10_000));
        assert_eq!(
            hardware.he_publications, 1,
            "observation must not republish the aggregate"
        );
        assert_ne!(
            tx.aggregate_slot_state(),
            TxSlotState::Free,
            "observation must retain DMA owners"
        );
    }
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn aggregate_uses_exact_ba_tid_and_defers_a_different_wmm_successor() {
    let (mut device, network) = make_network();
    send_ipv4_dscp_frame(&mut device, 1, 40);
    send_ipv4_dscp_frame(&mut device, 2, 46);
    send_ipv4_dscp_frame(&mut device, 3, 40);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(5, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(
        hardware.last_ht_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );
    let ConnectedTxActive::Aggregate(active) = &tx.active else {
        panic!("same-TID frames must own an aggregate");
    };
    assert_eq!(active.traffic.tid(), 5);
    assert_eq!(active.original_subframes, 1);
    assert_eq!(
        tx.ordinary.peek_qos_sequence(5),
        Some(SequenceNumber::new(8).unwrap())
    );
    assert_eq!(
        tx.ordinary.peek_qos_sequence(6),
        Some(SequenceNumber::new(7).unwrap())
    );
    assert_eq!(tx.prepared_network_frame_count(), 1);
    assert_eq!(
        network.tx_queue_len(),
        1,
        "the frame after the deferred TID stays FIFO-owned"
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b1));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(
        tx.start_prepared_network(&mut hardware, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    // The deferred Voice MPDU leaves alone at the rate control's HT data
    // schedule.
    assert_eq!(
        hardware.last_ht_queue,
        Some(LegacyTxQueue::Voice.hardware_index())
    );
    assert_eq!(
        tx.ordinary.peek_qos_sequence(6),
        Some(SequenceNumber::new(8).unwrap())
    );
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(network.tx_queue_len(), 1);

    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx());
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn negotiated_video_txop_bounds_he_aggregate_and_selects_video_queue() {
    let (mut device, network) = make_network();
    // EF requests VO, whose ACM bit in STANDARD_WMM forces the recovered
    // station downgrade to VI/UP5 before BlockAck and TXOP selection.
    send_ipv4_dscp_frame(&mut device, 1, 46);
    send_ipv4_dscp_frame(&mut device, 2, 46);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let mut ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    ordinary
        .policy_mut()
        .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
        .unwrap();
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(5, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(
        hardware.last_he_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );
    let ConnectedTxActive::Aggregate(active) = &tx.active else {
        panic!("negotiated video traffic must own an HE aggregate");
    };
    assert_eq!(active.traffic.tid(), 5);
    let AmpduTxConfig::He(config) = active.config else {
        panic!("HE rate must retain the HE publication config");
    };
    assert_eq!(config.txop_limit().units_32_us(), 94);

    hardware.aggregate_completion = Some(aggregate_completion(7, 0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.he_publications, 2);
    assert_eq!(
        tx.ordinary
            .policy()
            .contention_exponent(LegacyTxQueue::Video),
        4
    );
    assert_eq!(
        tx.ordinary
            .policy()
            .contention_exponent(LegacyTxQueue::Voice),
        2
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(
        tx.ordinary
            .policy()
            .contention_exponent(LegacyTxQueue::Video),
        3
    );
}

#[test]
fn he_aggregate_above_the_txop_threshold_uses_rts_and_survives_a_cts_timeout() {
    let (mut device, network) = make_network();
    send_ipv4_dscp_frame(&mut device, 1, 46);
    send_ipv4_dscp_frame(&mut device, 2, 46);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let mut ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    ordinary
        .policy_mut()
        .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
        .unwrap();
    // Two 32-us units leave no APEP budget after the preamble and BlockAck.
    ordinary.policy_mut().install_bss_protection(BssProtection {
        he_txop_rts_threshold: HeTxopDurationRtsThreshold::new(2),
        ..BssProtection::UNPROTECTED
    });
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(5, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    let rts = TxProtection::RtsCts {
        rate: LegacyRate::Ofdm24M,
    };
    let ConnectedTxActive::Aggregate(active) = &tx.active else {
        panic!("protected video traffic must still own an HE aggregate");
    };
    assert_eq!(active.config.control().protection, rts);
    let subframes = active.retry.current_subframes();
    assert_eq!(hardware.he_publications, 1);

    hardware.aggregate_completion = Some(MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(2, 0),
        0,
        7,
        u64::MAX,
        false,
    ));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.he_publications, 2);
    let ConnectedTxActive::Aggregate(active) = &tx.active else {
        panic!("a CTS timeout must retain the aggregate");
    };
    assert_eq!(active.retry.current_subframes(), subframes);
    assert_eq!(active.retry.protection_failures(), 1);
    assert_eq!(active.retry.acknowledged(), 0);
    assert_eq!(active.config.control().protection, rts);
    assert_eq!(hardware.legacy_publications, 0);

    // Every further protection exchange fails: the aggregate is kept and a
    // BlockAckReq for its head leaves on the video queue of TID 5.
    for _ in 1..oer_esp32s31_ieee80211_mac::tx::runtime::VENDOR_SHORT_RETRY_LIMIT {
        hardware.aggregate_completion = Some(MacHtAmpduCompletionObservation::new_model(
            MacTxCompletionObservation::new_model(2, 0),
            0,
            7,
            u64::MAX,
            false,
        ));
        assert_eq!(
            tx.service(
                &mut hardware,
                WifiTxWake::Interrupt {
                    events: EVENT_TX_COMPLETE,
                },
            ),
            Ok(WifiTxProgress::Pending)
        );
    }
    let ConnectedTxActive::RequestingBlockAck(active) = &tx.active else {
        panic!("an exhausted protection exchange must request the aggregate's BlockAck");
    };
    let head = active.retry.current_first_sequence();
    assert_eq!(hardware.legacy_publications, 1);
    assert_eq!(
        hardware.last_legacy_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );

    // The BlockAck acknowledges every MPDU: the aggregate is delivered.
    hardware.aggregate_completion = Some(MacHtAmpduCompletionObservation::new_model(
        MacTxCompletionObservation::new_model(0, 0),
        0,
        head.get(),
        u64::MAX,
        true,
    ));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(matches!(tx.active, ConnectedTxActive::Idle));
    let status = tx.last_aggregate_status.unwrap();
    assert_eq!(status.result, MacAmpduTxResult::Delivered);
    assert_eq!(u16::from(subframes), status.block_acknowledged_subframes);
}

#[test]
fn peer_advertised_tiny_he_txop_cannot_wrap_into_aggregate_capacity() {
    let (mut device, network) = make_network();
    send_ipv4_dscp_frame(&mut device, 1, 40);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let mut ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut tiny_txop = STANDARD_WMM;
    tiny_txop[20..22].copy_from_slice(&1_u16.to_le_bytes());
    ordinary
        .policy_mut()
        .install_wmm(parse_wmm_parameter_element(&tiny_txop).unwrap())
        .unwrap();
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(5, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    // No aggregate: one ordinary MPDU at the HE data schedule.
    assert_eq!(hardware.he_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert_eq!(hardware.ht_publications, 0);
    assert_eq!(
        hardware.last_he_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );
    assert_eq!(
        tx.ordinary.peek_qos_sequence(5),
        Some(SequenceNumber::new(8).unwrap())
    );
    assert!(
        observer.observed(AggregateTxObservation::NetworkSingleMpdu {
            reason: NetworkSingleMpduReason::FreshAggregateCapacity,
            ethernet_length: 18,
        })
    );
    hardware.ordinary_completion = Some(aggregate_completion(0, 0).tx());
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn negotiated_amsdu_pairs_network_frames_inside_the_block_ack_window() {
    const FRAME_CAPACITY: usize = 1_600;
    const HEADROOM: usize = TEST_HEADROOM;
    const TRAILER: usize = 1_632;
    const QUEUE_DEPTH: usize = 4;
    let (mut device, network) =
        make_network_config::<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>();
    for marker in 1..=4 {
        device
            .transmit(&mut context())
            .expect("free production-sized pinned network slot")
            .consume(1_514, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<4_096>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(HtRate::new(
                HtMcs::Mcs7,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz40,
            )),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_agreement(0, Some((TEST_SLOTS as u16, true)));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert_eq!(hardware.ht_publications, 1);
    assert_eq!(hardware.legacy_publications, 0);
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::QueueEmpty,
    }));
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
}

#[test]
fn aggregate_never_exceeds_the_peer_negotiated_block_ack_window() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    send_frame(&mut device, 3);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);

    // The local arena can hold three frames, but the peer accepted only two.
    tx.set_block_ack_window(0, Some(2));
    assert_eq!(tx.block_ack_window(0), Some(2));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert!(observer.observed(AggregateTxObservation::Prepared {
        subframes: 2,
        stop: AggregateBuildStop::FrameLimit,
    }));
    assert_eq!(network.tx_queue_len(), 1);

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete),
    );
}

#[test]
fn pipelined_arena_survives_current_retry_and_publishes_at_next_boundary() {
    const PIPELINE_DEPTH: usize = 6;
    let (mut device, network) =
        make_network_config::<TEST_FRAME_CAPACITY, TEST_HEADROOM, TEST_TRAILER, PIPELINE_DEPTH>();
    for marker in 1..=3 {
        device
            .transmit(&mut context())
            .expect("pipeline queue has one free slot")
            .consume(17, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 1);
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(network.tx_queue_len(), 0);

    // The network producer may wake more than once while the current arena is
    // hardware-owned. Each wake extends the same software-owned standby arena
    // without publishing it or consuming another sequence space.
    for marker in 4..=5 {
        device
            .transmit(&mut context())
            .expect("pipeline queue has one free slot")
            .consume(17, |frame| {
                frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, marker]);
                frame[6..12].copy_from_slice(&STATION);
                frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
                frame[14..].fill(marker);
            });
    }
    let standby_first = network.try_receive_tx_direct().unwrap();
    tx.prepare_network_standby(standby_first, &network.tx_consumer());
    assert!(tx.has_prepared_network_tx());
    assert_eq!(network.tx_queue_len(), 0);
    assert_eq!(hardware.ht_publications, 1);

    device
        .transmit(&mut context())
        .expect("pipeline queue has one free slot")
        .consume(17, |frame| {
            frame[..6].copy_from_slice(&[0x30, 0x31, 0x32, 0x33, 0x34, 6]);
            frame[6..12].copy_from_slice(&STATION);
            frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
            frame[14..].fill(6);
        });
    let extension = network.try_receive_tx_direct().unwrap();
    tx.prepare_network_standby(extension, &network.tx_consumer());
    assert!(tx.has_prepared_network_tx());
    assert_eq!(hardware.ht_publications, 1);
    assert_eq!(observer.count(AggregateTxObservation::StandbyPrepared), 1);
    assert_eq!(
        observer.count(AggregateTxObservation::Prepared {
            subframes: 3,
            stop: AggregateBuildStop::FrameLimit,
        }),
        1,
        "standby preparation stays private until the publication boundary"
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert!(tx.has_prepared_network_tx());

    hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert!(tx.has_prepared_network_tx());
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(
        tx.start_prepared_network(&mut hardware, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 3);
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(
        observer.count(AggregateTxObservation::Prepared {
            subframes: 3,
            stop: AggregateBuildStop::FrameLimit,
        }),
        2
    );

    hardware.aggregate_completion = Some(aggregate_completion(10, 0b111));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );

    let returned = tx
        .try_into_station_parts()
        .unwrap_or_else(|_| panic!("both aggregate arenas must return idle"));
    assert_eq!(returned.aggregate.primary().state(), TxSlotState::Free);
    assert_eq!(
        returned.aggregate.standby().map(|arena| arena.state()),
        Some(TxSlotState::Free)
    );
    let ConnectedTxSecurity::Wpa2Personal(key) = returned.security else {
        panic!("WPA2 test owner must return its pairwise key");
    };
    key.clear(&mut hardware);
}

#[test]
fn exhausted_ba_generation_invalidates_a_software_prepared_aggregate_before_publication() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::He(HeRate::new(HeMcs::Mcs9, HeGuardIntervalAndLtf::TwoLtf800Ns)),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    tx.block_ack_generations[0] = u32::MAX - 1;

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.he_publications, 1);

    send_frame(&mut device, 2);
    let standby_first = network.try_receive_tx_direct().unwrap();
    tx.prepare_network_standby(standby_first, &network.tx_consumer());
    assert!(tx.has_prepared_network_tx());

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b1));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );

    // Connected control owns this boundary and processes peer DELBA before
    // the saturated scheduler may publish a software-prepared network batch.
    tx.set_block_ack_window(0, None);
    assert_eq!(tx.block_ack_generation(0), Some(u32::MAX));
    // Reinstalling the same numeric agreement exhausts the checked monotonic
    // generation instead of wrapping to an alias of an old prepared token.
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(tx.block_ack_generations[0], u32::MAX);
    assert_eq!(tx.block_ack_generation(0), None);
    assert_eq!(tx.block_ack_window(0), None);
    assert!(!tx.block_ack_amsdu(0));
    assert_eq!(
        tx.start_prepared_network(&mut hardware, &network.tx_consumer()),
        Err(AggregateTxError::BlockAckAgreementChanged { tid: 0 })
    );
    assert_eq!(hardware.he_publications, 1);
    assert!(!tx.has_prepared_network_tx());
    assert_eq!(tx.queue_state(), MacTxQueueState::Ready);
    assert_eq!(tx.standby_aggregate_is_fully_free(), Some(true));

    tx.set_block_ack_window(0, None);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(tx.block_ack_generation(0), None);
    assert_eq!(tx.block_ack_window(0), None);
}

#[test]
fn ordinary_control_tx_cannot_admit_a_standby_aggregate() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);

    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        ConnectedControlTx::start_action(
            &mut tx,
            &mut hardware,
            &[3, 0],
            ActionTxConfig::VENDOR_MANAGEMENT,
        ),
        Ok(DatapathControlProgress::TxPending),
    );
    assert!(tx.active());
    assert!(!tx.can_prepare_network_tx());

    let frame = network.try_receive_tx_direct().unwrap();
    tx.prepare_network_standby(frame, &network.tx_consumer());
    assert!(!tx.has_prepared_network_tx());
}

#[test]
fn rejected_standby_preparation_preserves_the_hardware_owned_primary() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);

    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut primary = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut standby = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::pipelined(
            HtAmpduTxResources::new_model(primary.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
            HtAmpduTxResources::new_model(standby.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    let first = network.try_receive_tx_direct().unwrap();
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending),
    );
    assert!(tx.active());
    assert_eq!(tx.aggregate_slot_state(), TxSlotState::HardwareOwned);
    let next_sequence = tx.ordinary.peek_qos_sequence(0);

    // A malformed immediate successor stays at the FIFO boundary without
    // mutating aggregate metadata or erasing the live primary transaction.
    send_short_frame(&mut device);
    let short = network.try_receive_tx_direct().unwrap();
    tx.prepare_network_standby(short, &network.tx_consumer());

    assert!(tx.active());
    assert_eq!(tx.aggregate_slot_state(), TxSlotState::HardwareOwned);
    assert_eq!(tx.standby_aggregate_is_fully_free(), Some(true));
    assert_eq!(tx.ordinary.peek_qos_sequence(0), next_sequence);
    assert!(tx.has_prepared_network_tx());
    assert!(tx.standby_error.is_none());

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    assert_eq!(
        tx.start_prepared_network(&mut hardware, &network.tx_consumer()),
        Err(AggregateTxError::Ordinary(
            SingleMpduTxError::EthernetFrameTooShort
        ))
    );
    assert_eq!(tx.ordinary.peek_qos_sequence(0), next_sequence);
}

#[test]
fn aggregate_abort_retains_frames_until_deadline_and_quarantines_failed_detach() {
    use oer_esp32s31_ieee80211_mac::irq::EVENT_TX_TIMEOUT;

    for detach_succeeds in [true, false] {
        let (mut device, network) = make_network();
        send_frame(&mut device, 1);
        send_frame(&mut device, 2);
        let first = network.try_receive_tx_direct().unwrap();
        let mut hardware = Hardware {
            timeout_detach_succeeds: detach_succeeds,
            ..Hardware::default()
        };
        let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
        let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
        let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut tx = ConnectedTx::new_for_test(
            ordinary,
            AggregateTxResources::single(
                HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
                &mut retention,
            ),
            AggregateTxConfig {
                rate: TxPhyRate::Ht(TEST_RATE),
                frame_limit: TEST_SLOTS as u8,
                completion_timeout: oer_time::Duration::from_micros(250_000),
                he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            },
        )
        .unwrap();
        tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
        tx.start_network(&mut hardware, first, &network.tx_consumer())
            .unwrap();
        let completion_deadline = tx.next_deadline().unwrap();
        assert_eq!(
            tx.service(&mut hardware, WifiTxWake::Deadline),
            Ok(WifiTxProgress::Pending)
        );
        assert_eq!(tx.next_deadline(), Some(completion_deadline));
        assert_eq!(hardware.abort_requests, 0);

        let timeout = WifiTxWake::Interrupt {
            events: EVENT_TX_TIMEOUT,
        };
        let abort_started = tx.ordinary.now();
        assert_eq!(
            tx.service(&mut hardware, timeout),
            Ok(WifiTxProgress::Pending)
        );
        let deadline = tx.next_deadline().unwrap();
        assert_eq!(
            deadline,
            abort_started.checked_add(AMPDU_ABORT_SETTLE).unwrap()
        );
        assert_eq!(tx.ordinary.now(), abort_started);
        assert_eq!(tx.active_network_frame_count(), 2);
        assert_eq!(network.tx_consumer().promotion_capacity(), 1);
        tx = match tx.try_into_parts() {
            Err(owner) => owner,
            Ok(_) => panic!("settling DMA owner must not be handed off"),
        };
        hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
        embassy_futures::block_on(
            tx.ordinary
                .wait_until(oer_time::Instant::from_micros(deadline.as_micros() - 1)),
        );
        for wake in [
            timeout,
            WifiTxWake::Deadline,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ] {
            assert_eq!(tx.service(&mut hardware, wake), Ok(WifiTxProgress::Pending));
            assert_eq!(tx.next_deadline(), Some(deadline));
            assert_eq!(network.tx_consumer().promotion_capacity(), 1);
        }
        assert_eq!(hardware.abort_requests, 1);
        assert_eq!(hardware.timeout_detaches, 0);
        embassy_futures::block_on(tx.wait_deadline());
        assert_eq!(tx.ordinary.now().as_micros(), deadline.as_micros());
        let result = tx.service(&mut hardware, WifiTxWake::Deadline);
        assert_eq!(hardware.timeout_detaches, 1);
        assert_eq!(hardware.ht_publications, 1);
        if detach_succeeds {
            assert_eq!(result, Ok(WifiTxProgress::Complete));
            assert_eq!(
                tx.take_last_aggregate_status().unwrap().result,
                MacAmpduTxResult::HardwareTimeout
            );
            assert_eq!(network.tx_consumer().promotion_capacity(), TEST_QUEUE_DEPTH);
            assert!(tx.try_into_parts().is_ok());
        } else {
            assert!(result.is_err());
            assert_eq!(tx.ampdu.active().state(), TxSlotState::ResetRequired);
            assert!(tx.try_into_parts().is_err());
            assert_eq!(network.tx_consumer().promotion_capacity(), 1);
        }
    }
}

#[test]
fn block_ack_completion_releases_all_referenced_network_leases() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));

    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 1);
    // General packet owners return at promotion. The two radio-owned frames
    // consume physical SRAM, while a new software owner can queue
    // independently in the freed general pool.
    send_frame(&mut device, 3);
    assert_eq!(network.tx_queue_len(), 1);
    assert_eq!(network.tx_consumer().promotion_capacity(), 1);

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let work = tx.aggregate_work();
    let status = tx.take_last_aggregate_status().unwrap();
    assert_eq!(*observer.terminal.lock().unwrap(), [status]);
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(work.publications, 1);
    assert_eq!(work.mpdus, 2);
    assert!(work.psdu_bytes > 0);
    assert!(work.nominal_data_micros > 0);
    assert_eq!(work.unestimated_publications, 0);
    assert_eq!(
        Some(status),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Delivered,
            original_subframes: 2,
            aggregate_attempts: 1,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 2,
            individual_retries: MacIndividualRetries::NONE,
            ack_snr_db: Some(96),
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx_direct().unwrap());
    }
}

#[test]
fn partial_block_ack_retains_missing_frames_across_one_republication() {
    let (mut device, network) = make_network();
    for marker in 1..=3 {
        send_frame(&mut device, marker);
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(network.tx_consumer().promotion_capacity(), 0);
    assert!(device.transmit(&mut context()).is_some());

    hardware.aggregate_completion = Some(aggregate_completion(8, 0b11));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let work = tx.aggregate_work();
    let status = tx.take_last_aggregate_status().unwrap();
    assert_eq!(*observer.terminal.lock().unwrap(), [status]);
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(work.publications, 2);
    assert_eq!(work.mpdus, 5);
    assert!(work.psdu_bytes > 0);
    assert!(work.nominal_data_micros > 0);
    assert_eq!(work.unestimated_publications, 0);
    assert_eq!(
        Some(status),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Delivered,
            original_subframes: 3,
            aggregate_attempts: 2,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 3,
            individual_retries: MacIndividualRetries::NONE,
            ack_snr_db: Some(96),
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    send_frame(&mut device, 6);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx_direct().unwrap());
    }
}

#[test]
fn one_missing_wmm_ht_mpdu_keeps_tid_queue_sequence_and_pn_in_ordinary_retry() {
    let (mut device, network) = make_network();
    send_ipv4_dscp_frame(&mut device, 1, 40);
    send_ipv4_dscp_frame(&mut device, 2, 40);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(5, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(
        hardware.last_ht_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );
    assert_eq!(
        tx.peek_qos_sequence(5),
        Some(SequenceNumber::new(9).unwrap())
    );
    assert_eq!(
        tx.peek_qos_sequence(0),
        Some(SequenceNumber::new(7).unwrap())
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b01));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert!(observer.terminal.lock().unwrap().is_empty());
    assert_eq!(
        tx.peek_qos_sequence(5),
        Some(SequenceNumber::new(9).unwrap())
    );
    assert_eq!(
        tx.peek_qos_sequence(0),
        Some(SequenceNumber::new(7).unwrap())
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(
        hardware.last_ht_queue,
        Some(LegacyTxQueue::Video.hardware_index())
    );

    // The individual retry uses the private ordinary descriptor. Both
    // referenced network allocations have already crossed the safe
    // detach/release edge and can be filled again while it is in flight.
    send_frame(&mut device, 3);
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);

    hardware.ordinary_completion = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let aggregate = tx
        .take_last_aggregate_status()
        .expect("ordinary retry completes the logical aggregate exchange");
    assert_eq!(*observer.terminal.lock().unwrap(), [aggregate]);
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(aggregate.result, MacAmpduTxResult::Delivered);
    assert_eq!(aggregate.original_subframes, 2);
    assert_eq!(aggregate.aggregate_attempts, 1);
    assert_eq!(aggregate.aggregate_rate, TxPhyRate::Ht(TEST_RATE));
    assert_eq!(aggregate.block_acknowledged_subframes, 1);
    assert_eq!(aggregate.delivered_subframes(), 2);
    assert_eq!(aggregate.total_publication_attempts(), 2);
    assert_eq!(
        aggregate.individual_retries,
        MacIndividualRetries {
            transmitted: 1,
            failed: 0,
            attempts: 1,
            final_rate: Some(TxPhyRate::Ht(TEST_RATE)),
        }
    );
    assert!(matches!(
        tx.take_last_ordinary_outcome(),
        Some(SingleMpduTxOutcome::Success(_))
    ));
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx_direct().unwrap());
    }
}

#[test]
fn an_agreement_ended_mid_exchange_sends_the_missing_mpdus_individually() {
    let (mut device, network) = make_network();
    send_frame(&mut device, 1);
    send_frame(&mut device, 2);
    send_frame(&mut device, 3);
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 1);

    // DELBA while the aggregate is in the air: the BlockAck still arrives,
    // but the two missing MPDUs must not be aggregated again.
    tx.set_block_ack_window(0, None);
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b010));
    let interrupt = WifiTxWake::Interrupt {
        events: EVENT_TX_COMPLETE,
    };
    assert_eq!(
        tx.service(&mut hardware, interrupt),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(tx.take_last_aggregate_status(), None);

    // The first individual retry is acknowledged; the second starts.
    hardware.ordinary_completion = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        tx.service(&mut hardware, interrupt),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 3);
    assert_eq!(tx.take_last_aggregate_status(), None);

    hardware.ordinary_completion = Some(MacTxCompletionObservation::new_model(0, 0));
    assert_eq!(
        tx.service(&mut hardware, interrupt),
        Ok(WifiTxProgress::Complete)
    );
    let aggregate = tx
        .take_last_aggregate_status()
        .expect("the last individual retry completes the logical exchange");
    assert_eq!(*observer.terminal.lock().unwrap(), [aggregate]);
    assert_eq!(aggregate.result, MacAmpduTxResult::Delivered);
    assert_eq!(aggregate.original_subframes, 3);
    assert_eq!(aggregate.aggregate_attempts, 1);
    assert_eq!(aggregate.block_acknowledged_subframes, 1);
    assert_eq!(aggregate.individual_retries.transmitted, 2);
    assert_eq!(aggregate.delivered_subframes(), 3);
    assert!(!tx.active());
    for _ in 0..3 {
        drop(network.try_receive_tx_direct());
    }
}
