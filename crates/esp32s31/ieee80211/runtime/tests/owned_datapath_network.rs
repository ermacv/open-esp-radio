use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_embassy_net_owned::{LinkState, NetworkInterfaceId, OwnedEndpointResources};
use oer_esp32s31_ieee80211_runtime::datapath::{PinnedTxPool, PinnedTxResources, SoftwareTxFrame};
use oer_esp32s31_ieee80211_runtime::datapath::{
    network::DatapathNetwork,
    owned::{DualOwnedDatapathNetwork, OwnedDatapathNetwork},
};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

static RX_STAGE_POOL: oer_esp32s31_ieee80211_mac::rx::pool::RxStagePool<
    { oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_SLOT_COUNT },
    { oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_PAYLOAD_CAPACITY },
> = oer_esp32s31_ieee80211_mac::rx::pool::RxStagePool::new();
static RX_ZERO_COPY_ORIGIN: oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin =
    oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin::new(
        &RX_ZERO_COPY_ORIGIN,
        RX_STAGE_POOL.external_handoff_pool(),
        16,
    );

#[path = "owned_datapath_network/airtime.rs"]
mod airtime;

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn packet(allocator: PacketBufAllocator, marker: u8) -> PacketBuf {
    let mut packet = allocator.try_alloc().expect("test packet credit");
    packet.set_len(14);
    packet.fill(marker);
    packet
}

struct AlternateSoftwareFrame {
    interface: NetworkInterfaceId,
    ethernet: [u8; 14],
}

impl SoftwareTxFrame for AlternateSoftwareFrame {
    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    fn ethernet(&self) -> &[u8] {
        &self.ethernet
    }
}

#[test]
fn physical_materializer_accepts_a_non_xarxa_software_owner() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const TX_QUEUE_DEPTH: usize = 1;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let interface = NetworkInterfaceId::new(3);
    let resources = Box::leak(Box::new(PhysicalResources::new()));
    let pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let materializer = resources.split(pool).for_interface(interface);
    let frame = AlternateSoftwareFrame {
        interface,
        ethernet: [0x7a; 14],
    };

    let physical = materializer
        .try_materialize(frame)
        .unwrap_or_else(|_| panic!("one physical credit is available"));
    assert_eq!(physical.as_slice(), &[0x7a; 14]);
}

#[test]
fn owned_backlog_and_physical_dma_credits_remain_independent() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const RX_QUEUE_DEPTH: usize = 1;
    const NETWORK_TX_DEPTH: usize = 2;
    const TX_QUEUE_DEPTH: usize = 1;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let general = allocator::<NETWORK_TX_DEPTH>();
    let rx = allocator::<RX_QUEUE_DEPTH>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
    >::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) = endpoint.split(interface, [2, 0, 0, 0, 0, 1], rx);

    let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
    let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let physical = physical_resources.split(physical_pool);
    let network = OwnedDatapathNetwork::new(owned, physical);
    network.set_link_state(interface, LinkState::Up);

    device.transmit(packet(general, 0x41)).unwrap();
    device.transmit(packet(general, 0x42)).unwrap();
    assert_eq!(network.tx_queue_len(interface), NETWORK_TX_DEPTH);

    let first = network.try_receive_tx(interface).expect("first owned TX");
    assert!(
        !device.can_transmit(),
        "selection retains software admission"
    );
    let physical = network.tx_consumer(interface);
    let first = physical
        .try_promote(first)
        .unwrap_or_else(|_| panic!("one physical SRAM credit is initially free"));
    assert!(
        device.can_transmit(),
        "promotion releases software admission before DMA completion"
    );

    let second = network.try_receive_tx(interface).expect("second owned TX");
    let second = match physical.try_promote(second) {
        Ok(_) => panic!("the retained first frame owns the only SRAM credit"),
        Err(second) => second,
    };
    assert_eq!(second.as_slice(), &[0x42; 14]);
    let returned_first = general
        .try_alloc()
        .expect("successful promotion released only its source owner");
    assert!(general.try_alloc().is_none());

    drop(first);
    let second = physical
        .try_promote(second)
        .unwrap_or_else(|_| panic!("completion returns the physical credit"));
    assert_eq!(second.as_slice(), &[0x42; 14]);
    assert!(general.try_alloc().is_some());
    drop(returned_first);
}

#[test]
fn datapath_reserves_sram_before_removing_a_software_owner() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const RX_QUEUE_DEPTH: usize = 1;
    const NETWORK_TX_DEPTH: usize = 2;
    const TX_QUEUE_DEPTH: usize = 1;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let general = allocator::<NETWORK_TX_DEPTH>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
    >::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) =
        endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator::<RX_QUEUE_DEPTH>());
    let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
    let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let network = OwnedDatapathNetwork::new(owned, physical_resources.split(physical_pool));
    network.set_link_state(interface, LinkState::Up);

    device.transmit(packet(general, 0x51)).unwrap();
    device.transmit(packet(general, 0x52)).unwrap();
    let consumer = network.tx_consumer(interface);
    let first = consumer
        .try_receive_direct()
        .expect("one free SRAM slot promotes the first owner");
    assert_eq!(consumer.queue_len(), 1);

    assert!(consumer.try_receive_direct().is_none());
    assert_eq!(
        consumer.queue_len(),
        1,
        "physical exhaustion must retain the next general-memory owner"
    );

    drop(first);
    let second = consumer
        .try_receive_direct()
        .expect("terminal return makes the retained owner promotable");
    assert_eq!(second.as_slice(), &[0x52; 14]);
}

#[test]
fn physical_batch_admission_never_moves_a_partial_prefix() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const RX_QUEUE_DEPTH: usize = 1;
    const NETWORK_TX_DEPTH: usize = 2;
    const TX_QUEUE_DEPTH: usize = 1;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let general = allocator::<NETWORK_TX_DEPTH>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        RX_QUEUE_DEPTH,
        NETWORK_TX_DEPTH,
    >::new()));
    let interface = NetworkInterfaceId::new(0);
    let (mut device, owned) =
        endpoint.split(interface, [2, 0, 0, 0, 0, 1], allocator::<RX_QUEUE_DEPTH>());
    let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
    let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let network = OwnedDatapathNetwork::new(owned, physical_resources.split(physical_pool));
    network.set_link_state(interface, LinkState::Up);

    device.transmit(packet(general, 0x61)).unwrap();
    device.transmit(packet(general, 0x62)).unwrap();
    let consumer = network.tx_consumer(interface);
    let mut sources = [
        Some(network.try_receive_tx(interface).expect("first owner")),
        Some(network.try_receive_tx(interface).expect("second owner")),
    ];
    let mut destinations = [None, None];

    assert!(!consumer.try_promote_batch(&mut sources, &mut destinations));
    assert!(
        !device.can_transmit(),
        "failed batch retains every software admission"
    );
    assert!(destinations.iter().all(Option::is_none));
    assert_eq!(sources[0].as_ref().unwrap().as_slice(), &[0x61; 14]);
    assert_eq!(sources[1].as_ref().unwrap().as_slice(), &[0x62; 14]);
    assert!(general.try_alloc().is_none());
}

#[test]
fn dual_owned_endpoints_keep_logical_backlogs_separate_from_one_dma_horizon() {
    const FRAME_CAPACITY: usize = 64;
    const HEADROOM: usize = 16;
    const TRAILER: usize = 8;
    const RX_QUEUE_DEPTH: usize = 1;
    const NETWORK_TX_DEPTH: usize = 2;
    const TX_QUEUE_DEPTH: usize = 1;
    type Endpoint = OwnedEndpointResources<NoopRawMutex, RX_QUEUE_DEPTH, NETWORK_TX_DEPTH>;
    type PhysicalPool = PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;
    type PhysicalResources =
        PinnedTxResources<NoopRawMutex, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>;

    let station_interface = NetworkInterfaceId::new(0);
    let access_point_interface = NetworkInterfaceId::new(1);
    let station_endpoint = Box::leak(Box::new(Endpoint::new()));
    let access_point_endpoint = Box::leak(Box::new(Endpoint::new()));
    let (mut station_device, station_radio) = station_endpoint.split(
        station_interface,
        [2, 0, 0, 0, 0, 1],
        allocator::<RX_QUEUE_DEPTH>(),
    );
    let (mut access_point_device, access_point_radio) = access_point_endpoint.split(
        access_point_interface,
        [2, 0, 0, 0, 0, 2],
        allocator::<RX_QUEUE_DEPTH>(),
    );
    let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
    let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
    let physical = physical_resources.split(physical_pool);
    let network = DualOwnedDatapathNetwork::new(
        station_radio,
        access_point_radio,
        physical,
        &RX_ZERO_COPY_ORIGIN,
    );

    network.set_link_state(station_interface, LinkState::Up);
    assert!(station_device.link_is_up());
    assert!(!access_point_device.link_is_up());
    network.set_link_state(access_point_interface, LinkState::Up);

    let station_general = allocator::<1>();
    let access_point_general = allocator::<2>();
    station_device
        .transmit(packet(station_general, 0x51))
        .unwrap();
    access_point_device
        .transmit(packet(access_point_general, 0x61))
        .unwrap();
    access_point_device
        .transmit(packet(access_point_general, 0x62))
        .unwrap();

    assert_eq!(network.tx_queue_len(station_interface), 1);
    assert_eq!(network.tx_queue_len(access_point_interface), 2);

    let station = network
        .try_receive_tx(station_interface)
        .expect("station owns its queue head");
    let access_point = network
        .try_receive_tx(access_point_interface)
        .expect("access point owns its queue head");
    assert_eq!(station.as_slice(), &[0x51; 14]);
    assert_eq!(access_point.as_slice(), &[0x61; 14]);

    let physical = network.tx_consumer(access_point_interface);
    let promoted = physical
        .try_promote(access_point)
        .unwrap_or_else(|_| panic!("the shared physical horizon accepts one selected AP owner"));
    let remaining = network
        .try_receive_tx(access_point_interface)
        .expect("the second AP owner remains independently selectable");
    assert_eq!(remaining.as_slice(), &[0x62; 14]);
    assert!(physical.try_promote(remaining).is_err());

    drop(station);
    drop(promoted);
    assert!(station_general.try_alloc().is_some());
    assert!(access_point_general.try_alloc().is_some());
}
