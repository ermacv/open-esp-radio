//! Production queue/materializer boundaries with the portable reservation owner.
//! Fixed synthetic service costs test ownership, not a radio timing estimate.

use super::*;
use core::num::NonZeroU32;
use oer_ieee80211_datapath::{
    SelectedBurstMaterializer,
    airtime::{AirtimeCandidate, AirtimeError, AirtimeStorage},
};
use oer_ieee80211_softmac::MacTxWork;

#[test]
fn airtime_software_and_physical_credits_have_independent_release_edges() {
    type Pool = PinnedTxPool<64, 16, 8, 1>;
    type Physical = PinnedTxResources<NoopRawMutex, 64, 16, 8, 1>;
    let general = allocator::<2>();
    let mut endpoint = OwnedEndpointResources::<NoopRawMutex, 1, 2>::new();
    let interface = NetworkInterfaceId::new(0);
    let (mut device, radio) = endpoint.split(interface, [2; 6], allocator::<1>());
    let physical_resources = Box::leak(Box::new(Physical::new()));
    let pool = Pool::pin_static(Box::leak(Box::new(Pool::new())));
    let physical_consumer = physical_resources.split(pool);
    let blocker = physical_consumer
        .for_interface(interface)
        .try_materialize(AlternateSoftwareFrame {
            interface,
            ethernet: [0; 14],
        })
        .unwrap_or_else(|_| panic!("one physical credit"));
    let network = OwnedDatapathNetwork::new(radio, physical_consumer);
    network.set_link_state(interface, LinkState::Up);
    let consumer = network.tx_consumer(interface);
    let queues = consumer.destination_queues().unwrap();
    for peer in [4, 6] {
        device.transmit(packet(general, peer)).unwrap();
    }
    let cost = NonZeroU32::new(100).unwrap();
    let candidates = || {
        [4, 6].map(|peer| {
            let head = queues.head_for([peer; 6]).unwrap();
            assert_eq!((head.ethernet_bytes, head.pending_frames), (14, 1));
            AirtimeCandidate {
                // A stable test association generation; queue metadata does not
                // assign it on behalf of the radio.
                key: ([peer; 6], 7_u32),
                minimum_micros: cost,
            }
        })
    };
    let mut storage = AirtimeStorage::<_, 2, 1>::new(cost);
    let mut scheduler = storage.scheduler();
    let grant = scheduler.reserve(candidates()).unwrap().unwrap();
    assert_eq!(consumer.materialization_capacity(), 0);
    scheduler.cancel(grant).unwrap();
    assert_eq!(consumer.queue_len(), 2);
    assert!(!device.can_transmit());
    assert!(general.try_alloc().is_none());

    drop(blocker);
    let grant = scheduler.reserve(candidates()).unwrap().unwrap();
    let selected = grant.key();
    let frame = queues.try_take_for(selected.0).unwrap();
    assert!(!device.can_transmit(), "claim transfers software admission");
    let physical = consumer
        .try_promote(frame)
        .unwrap_or_else(|_| panic!("physical release makes selected work admissible"));
    let in_flight = grant.published();
    assert_eq!(physical.as_slice(), &[selected.0[0]; 14]);
    assert!(
        device.can_transmit(),
        "materialization returns the software owner"
    );
    assert_eq!(consumer.materialization_capacity(), 0);
    assert!(matches!(
        scheduler.reserve([]),
        Err(AirtimeError::ReservationCapacity)
    ));
    // Simulated publications exercise budget ownership with a real DMA lease.
    // They are not evidence that a descriptor was submitted to hardware.
    let mut work = MacTxWork::new();
    for _ in 0..3 {
        work.record(14, 1, None);
        assert_eq!(consumer.materialization_capacity(), 0);
        assert!(matches!(
            scheduler.reserve([]),
            Err(AirtimeError::ReservationCapacity)
        ));
    }
    // A hardware owner must first prove terminal detach. This host fixture
    // never submits DMA, so it can release its physical lease directly.
    let completion = in_flight.completed(work);
    drop(physical);
    assert_eq!(consumer.materialization_capacity(), 1);
    assert!(completion.work().estimated_exchange_micros(0).is_none());
    assert!(
        matches!(
            scheduler.reserve([]),
            Err(AirtimeError::ReservationCapacity)
        ),
        "physical release cannot invent an airtime settlement"
    );
    // An explicit synthetic charge resolves the unknown cost in this fixture.
    let retained_work = scheduler
        .settle(completion, NonZeroU32::new(300).unwrap())
        .unwrap();
    assert_eq!(retained_work, work);
    assert_eq!(retained_work.publications, 3);
    assert_eq!(scheduler.balance_micros(selected), Some(-200));
    let other = if selected.0 == [4; 6] { [6; 6] } else { [4; 6] };
    assert_eq!(queues.head_for(other).unwrap().pending_frames, 1);
    drop(queues.try_take_for(other));
}
