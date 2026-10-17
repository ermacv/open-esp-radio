//! Zero-copy RX through detached DMA slots, including a consumer which never
//! returns what it received.

use core::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_embassy_net_owned::{
    ExternalRxOrigin, ExternalRxRefusal, NetworkInterfaceId, OwnedEndpointResources,
    OwnedRxPublisher, RxEnqueueError,
};
use oer_memory::{ExternalRxBuffer, ExternalRxHandoffPool, ExternalRxRadioLease};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

const SLOTS: usize = 32;
const CAPACITY: usize = 1_700;
/// One BA-16 receive burst staged while the network holds its full cap.
const RESERVE: usize = 16;
const CAP: usize = SLOTS - RESERVE;
const FRAME_LEN: usize = 60;
/// A typical 802.11 QoS + CCMP + LLC prefix rewritten to Ethernet in place.
const ETHERNET_OFFSET: usize = 34 - 14 + 8;

#[repr(C, align(4))]
struct DmaBuffer([u8; CAPACITY]);

/// Simulated RX DMA ring: counts buffers the handoff pool gave back.
struct DmaRing {
    returned: AtomicUsize,
}

// SAFETY: touches only the ring's atomic counter, from any thread.
#[allow(unsafe_code, reason = "test DMA owner release edge")]
unsafe fn return_to_ring(owner: NonNull<()>, _index: usize) {
    // SAFETY: every buffer is constructed with a leaked `DmaRing` as owner.
    let ring = unsafe { owner.cast::<DmaRing>().as_ref() };
    ring.returned.fetch_add(1, Ordering::Relaxed);
}

struct Radio {
    ring: &'static DmaRing,
    pool: &'static ExternalRxHandoffPool<CAPACITY, SLOTS>,
    origin: &'static ExternalRxOrigin<CAPACITY, SLOTS>,
    detached: usize,
}

/// Each test binds its own statics: an origin names its own address.
macro_rules! radio {
    () => {{
        static POOL: ExternalRxHandoffPool<CAPACITY, SLOTS> = ExternalRxHandoffPool::new();
        static ORIGIN: ExternalRxOrigin<CAPACITY, SLOTS> =
            ExternalRxOrigin::new(&ORIGIN, &POOL, RESERVE);
        Radio {
            ring: Box::leak(Box::new(DmaRing {
                returned: AtomicUsize::new(0),
            })),
            pool: &POOL,
            origin: &ORIGIN,
            detached: 0,
        }
    }};
}

/// Outcome of one frame through the staged publication path.
#[derive(Debug, Eq, PartialEq)]
enum Staged {
    /// No free handoff slot: the driver keeps the frame in its ring.
    NoSlot,
    /// Admission refused before any rewrite; the frame was copied.
    Refused(ExternalRxRefusal, Result<(), RxEnqueueError>),
    /// Admitted and published through `publish_external`.
    Published(Result<(), RxEnqueueError>),
}

impl Radio {
    /// Detach one fresh DMA buffer holding a frame at `offset` and stage it.
    fn stage(
        &mut self,
        offset: usize,
        marker: u8,
    ) -> Option<ExternalRxRadioLease<'static, CAPACITY>> {
        let index = self.detached;
        self.detached += 1;
        let buffer: &'static mut DmaBuffer = Box::leak(Box::new(DmaBuffer([0; CAPACITY])));
        // SAFETY (test): the leaked buffer is used only through this binding
        // until the pool returns it.
        #[allow(unsafe_code, reason = "test DMA owner detaches its buffer")]
        let buffer = unsafe {
            ExternalRxBuffer::new(
                NonNull::from(buffer).cast::<u8>(),
                CAPACITY,
                CAPACITY,
                NonNull::from(self.ring).cast(),
                index,
                return_to_ring,
            )
        };
        let mut lease = self.pool.try_claim_radio(buffer, index % SLOTS).ok()?;
        lease.with_frame(|bytes| {
            let frame = &mut bytes[offset..offset + FRAME_LEN];
            frame.fill(marker);
            frame[12..14].copy_from_slice(&[0x08, 0x00]);
        });
        Some(lease)
    }

    /// The radio's `publish_staged` for one data frame.
    fn receive<M: embassy_sync::blocking_mutex::raw::RawMutex, const Q: usize>(
        &mut self,
        publisher: &OwnedRxPublisher<'_, M, Q>,
        offset: usize,
        marker: u8,
    ) -> Staged {
        let Some(lease) = self.stage(offset, marker) else {
            return Staged::NoSlot;
        };
        match publisher.try_admit_external(self.origin) {
            Err(refusal) => {
                let result = publisher.try_send(&lease.frame()[offset..offset + FRAME_LEN]);
                drop(lease);
                Staged::Refused(refusal, result)
            }
            Ok(admission) => {
                let index = lease.republish(offset, FRAME_LEN);
                Staged::Published(publisher.publish_external(admission, index))
            }
        }
    }

    /// A management frame consumed by the role: staged, read, released.
    fn management(&mut self) -> bool {
        match self.stage(0, 0xee) {
            Some(lease) => {
                assert_eq!(lease.frame()[0], 0xee);
                true
            }
            None => false,
        }
    }

    fn free_slots(&self) -> usize {
        SLOTS - self.pool.claimed_slots()
    }
}

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

#[test]
fn a_consumer_which_never_drains_stops_at_the_cap_and_management_still_flows() {
    let mut radio = radio!();
    let endpoint = Box::leak(Box::new(
        OwnedEndpointResources::<NoopRawMutex, 64, 1>::new(),
    ));
    let (mut device, runner) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<4>());
    runner.link_controller().set_link_up(true);
    let publisher = runner.rx_publisher();
    // The socket: receives every frame and never returns one.
    let mut socket: Vec<PacketBuf> = Vec::new();

    for sequence in 0..200_u32 {
        let marker = sequence as u8;
        let staged = radio.receive(&publisher, ETHERNET_OFFSET, marker);
        let counters = radio.origin.counters();
        if sequence < CAP as u32 {
            assert_eq!(staged, Staged::Published(Ok(())), "frame {sequence}");
        } else {
            let Staged::Refused(ExternalRxRefusal::OverCap, _) = staged else {
                panic!("frame {sequence} beyond the cap was {staged:?}");
            };
        }
        while let Some(packet) = device.receive() {
            socket.push(packet);
        }
        // The pool invariant: the network never holds more than its cap, so
        // the driver keeps a full burst of free slots.
        assert!(counters.held <= CAP);
        assert!(radio.free_slots() >= RESERVE, "frame {sequence}");
        // Management keeps its slots and returns them to the ring.
        let returned = radio.ring.returned.load(Ordering::Relaxed);
        assert!(
            radio.management(),
            "management frame {sequence} found no slot"
        );
        assert_eq!(radio.ring.returned.load(Ordering::Relaxed), returned + 1);
    }

    let counters = radio.origin.counters();
    assert_eq!(counters.adopted, CAP as u32);
    assert_eq!(counters.held, CAP);
    assert_eq!(counters.peak_held, CAP);
    assert_eq!(counters.copied_over_cap, 200 - CAP as u32);
    assert_eq!(counters.dropped, 0);
    // Four copies fit the RX pool; the socket retains them as well.
    assert_eq!(socket.len(), CAP + 4);
    let adopted = socket
        .iter()
        .filter(|packet| radio.origin.owns(packet))
        .count();
    assert_eq!(adopted, CAP);
    for packet in socket.iter().filter(|packet| radio.origin.owns(packet)) {
        assert_eq!(packet.len(), FRAME_LEN);
        assert_eq!(&packet[12..14], &[0x08, 0x00]);
    }

    // Draining the socket returns every DMA buffer, then every credit.
    drop(socket);
    assert_eq!(radio.origin.counters().held, 0);
    assert_eq!(radio.pool.claimed_slots(), 0);
    assert_eq!(radio.ring.returned.load(Ordering::Relaxed), radio.detached);
    // The released slots are adopted again.
    assert_eq!(
        radio.receive(&publisher, ETHERNET_OFFSET, 1),
        Staged::Published(Ok(()))
    );
    assert_eq!(device.receive().map(|packet| packet[0]), Some(1));
}

#[test]
fn storage_without_a_full_packet_view_is_copied_and_released() {
    let mut radio = radio!();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 4, 1>::new()));
    let (mut device, runner) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    runner.link_controller().set_link_up(true);
    let publisher = runner.rx_publisher();
    // 1700 - 187 < PACKET_BUF_SIZE: the Ethernet start is past the 186-byte
    // limit, so the adopted view would overrun the DMA buffer.
    let offset = CAPACITY - xarxa_driver::config::PACKET_BUF_SIZE + 1;
    assert_eq!(
        radio.receive(&publisher, offset, 7),
        Staged::Published(Ok(()))
    );
    let counters = radio.origin.counters();
    assert_eq!(
        (counters.adopted, counters.copied_unfit, counters.held),
        (0, 1, 0)
    );
    assert_eq!(radio.pool.claimed_slots(), 0);
    let packet = device.receive().unwrap();
    assert!(!radio.origin.owns(&packet));
    assert_eq!(packet[0], 7);
}

#[test]
fn a_full_queue_after_admission_is_a_counted_drop_which_returns_the_slot() {
    let mut radio = radio!();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (_device, runner) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    runner.link_controller().set_link_up(true);
    let publisher = runner.rx_publisher();
    let first = radio.stage(ETHERNET_OFFSET, 1).unwrap();
    let second = radio.stage(ETHERNET_OFFSET, 2).unwrap();
    let first_admission = publisher.try_admit_external(radio.origin).unwrap();
    let second_admission = publisher.try_admit_external(radio.origin).unwrap();
    let first = first.republish(ETHERNET_OFFSET, FRAME_LEN);
    let second = second.republish(ETHERNET_OFFSET, FRAME_LEN);
    assert_eq!(publisher.publish_external(first_admission, first), Ok(()));
    assert_eq!(
        publisher.publish_external(second_admission, second),
        Err(RxEnqueueError::QueueFull)
    );
    let counters = radio.origin.counters();
    assert_eq!(
        (counters.adopted, counters.dropped, counters.held),
        (1, 1, 1)
    );
    assert_eq!(radio.pool.claimed_slots(), 1);
    assert_eq!(
        publisher.try_admit_external(radio.origin).err(),
        Some(ExternalRxRefusal::QueueFull)
    );
}

#[test]
fn a_refused_or_unused_admission_holds_no_credit() {
    let radio = radio!();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (_device, runner) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    let publisher = runner.rx_publisher();
    assert_eq!(
        publisher.try_admit_external(radio.origin).err(),
        Some(ExternalRxRefusal::LinkDown)
    );
    runner.link_controller().set_link_up(true);
    drop(publisher.try_admit_external(radio.origin).unwrap());
    let counters = radio.origin.counters();
    assert_eq!(
        (counters.held, counters.peak_held, counters.copied_over_cap),
        (0, 1, 0)
    );
}

#[test]
fn an_interval_reports_its_own_counts_and_peak() {
    let mut radio = radio!();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 8, 1>::new()));
    let (mut device, runner) = endpoint.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    runner.link_controller().set_link_up(true);
    let publisher = runner.rx_publisher();
    for marker in 0..3 {
        radio.receive(&publisher, ETHERNET_OFFSET, marker);
    }
    let mut retained: Vec<PacketBuf> = core::iter::from_fn(|| device.receive()).collect();
    // The consumer keeps one packet and returns two.
    retained.truncate(1);
    let earlier = radio.origin.counters();
    radio.origin.restart_peak();
    radio.receive(&publisher, ETHERNET_OFFSET, 9);
    let interval = radio.origin.counters().wrapping_delta_since(earlier);
    assert_eq!(
        (interval.adopted, interval.held, interval.peak_held),
        (1, 2, 2)
    );
    assert_eq!(earlier.peak_held, 3);
}
