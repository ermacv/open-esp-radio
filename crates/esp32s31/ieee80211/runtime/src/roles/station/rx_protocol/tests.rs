use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_mac::rx::RxIngressConfig;

use oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxConfig;

use oer_ieee80211_mac::security::LinkProtection;

use std::boxed::Box;

use super::*;

struct Sink;

impl ConnectedRxSink for Sink {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

impl<const CAPACITY: usize, const SLOTS: usize> ConnectedRxProtocolSink<CAPACITY, SLOTS> for Sink {
    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        ready(())
    }
}

fn open_config() -> ConnectedRxConfig {
    ConnectedRxConfig {
        station_address: [2, 3, 4, 5, 6, 7],
        bssid: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        association_id: 1,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        security: LinkProtection::Open,
        peer_qos: false,
        management_protection: false,
    }
}

#[test]
#[should_panic(expected = "connected RX protocol requires a configured dispatcher epoch")]
fn protocol_rejects_an_unconfigured_static_dispatcher_arena() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));

    let _ = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
}

#[test]
#[should_panic(expected = "stopped connected RX protocol cannot service another turn")]
fn stopped_protocol_cannot_dispatch_with_a_parked_dispatcher() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let mut protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );

    protocol.shutdown_discard();
    embassy_futures::block_on(protocol.service_bounded(1));
}

#[test]
fn empty_standalone_queue_can_be_replaced_without_rebuilding_station_protocol() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );

    let processor = protocol
        .try_into_processor()
        .unwrap_or_else(|_| panic!("an empty standalone queue must detach"));
    drop(sender);
    let (_sender, receiver) = queue.split();
    let protocol = ConnectedReceiveProtocol::from_processor(receiver, processor);

    assert_eq!(protocol.queue_len(), 0);
    let stopped = protocol.into_stopped();
    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
}

#[test]
fn paired_processor_returns_the_same_stopped_protocol_owner() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let runtime_ptr = runtime as *mut _;
    let protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );

    let processor = protocol
        .try_into_processor()
        .unwrap_or_else(|_| panic!("empty standalone queue must detach"));
    let stopped = processor.into_stopped();

    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
    let (_, _, returned_runtime) = stopped.into_parts();
    assert_eq!(returned_runtime as *mut _, runtime_ptr);
}

#[test]
fn direct_and_async_reorder_paths_share_one_first_frame_marker() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
    let mut processor = protocol
        .try_into_processor()
        .unwrap_or_else(|_| panic!("an empty standalone queue must detach"));

    processor.runtime.reorder_first_starts[0] = Some(SequenceNumber::new(7).unwrap());
    processor.observe_first_reorder_frame(0, 0, SequenceNumber::new(8).unwrap());
    assert_eq!(processor.runtime.reorder_first_starts[0], None);
    processor.observe_first_reorder_frame(0, 0, SequenceNumber::new(9).unwrap());
    assert_eq!(processor.runtime.reorder_first_starts[0], None);
}

#[test]
fn stop_edge_returns_an_empty_reusable_protocol_epoch() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (_sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    // Host tests leak the arena to model the static lifetime required by an
    // embedded composition root without consuming thread-stack space.
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let runtime_ptr = runtime as *mut _;
    let runtime_size = core::mem::size_of_val(&*runtime);
    let mpdu_ptr = mpdu.as_mut_ptr();
    let ethernet_ptr = ethernet.as_mut_ptr();
    let protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
    assert!(
        core::mem::size_of_val(&protocol) < runtime_size,
        "the movable protocol handle must not absorb its static reorder arena"
    );
    assert!(
        core::mem::size_of_val(&protocol) <= 256,
        "the movable protocol handle must remain pointer-sized state, not absorb the dispatcher"
    );

    let stopped = protocol.into_stopped();
    assert_eq!(stopped.shutdown(), ConnectedRxProtocolShutdown::default());
    let (returned_mpdu, returned_ethernet, returned_runtime) = stopped.into_parts();
    assert_eq!(returned_mpdu.as_mut_ptr(), mpdu_ptr);
    assert_eq!(returned_mpdu.len(), 64);
    assert_eq!(returned_ethernet.as_mut_ptr(), ethernet_ptr);
    assert_eq!(returned_ethernet.len(), 64);
    assert_eq!(returned_runtime as *mut _, runtime_ptr);
}

#[test]
fn stopped_protocol_arena_starts_a_second_epoch_without_reinitialization() {
    let queue = StagedRxQueue::<NoopRawMutex, 1, 64, 1>::new();
    let (sender, receiver) = queue.split();
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0_u8; 64];
    let mut ethernet = [0_u8; 64];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    let runtime_ptr = runtime as *mut _;
    let scratch_ptrs = (mpdu.as_mut_ptr(), ethernet.as_mut_ptr());

    let first = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
    let stopped = first.into_stopped();
    let (mpdu, ethernet, runtime) = stopped.into_parts();
    assert!(!runtime.dispatcher_configured());
    runtime.try_reconfigure_dispatcher(open_config()).unwrap();
    assert!(runtime.dispatcher_configured());

    drop(sender);
    let (_sender, receiver) = queue.split();
    let second = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        Sink,
        mpdu,
        ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
    let stopped = second.into_stopped();
    let (mpdu, ethernet, runtime) = stopped.into_parts();

    assert_eq!(mpdu.as_mut_ptr(), scratch_ptrs.0);
    assert_eq!(ethernet.as_mut_ptr(), scratch_ptrs.1);
    assert_eq!(runtime as *mut _, runtime_ptr);
    assert!(!runtime.dispatcher_configured());
}
