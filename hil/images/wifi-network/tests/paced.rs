//! The load image's paced sender on real sockets: the image's facade
//! (`udp.rs`) and its schedule (`load/pace.rs`, with its `CATCH_UP` and
//! batches), over a port that takes every frame and a peer that answers ARP,
//! at the stand's paced level: about nine datagrams a millisecond, a burst
//! the half look-ahead covers.
//! `oer` and the load service stand in for their chip-side names.
extern crate self as oer;
extern crate self as oer_hil_services_load;

use core::cell::Cell;
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Wake;

use embassy_futures::yield_now;
use oer_portable_events::EventSink;
use oer_portable_time::{Clock, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{Group, Interface, NetEvent, NetworkOwner, claim};
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Unpublished};
use oer_protocols_network_runtime::{
    Direct, MemoryPolicy, RequestKind, Service, ServiceEvent, SocketLanes, StopLane,
};
use oer_protocols_network_wire::arp::{self, Arp, Operation};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::Dscp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaticAddress {
    pub address: [u8; 4],
    pub prefix: u8,
    pub gateway: [u8; 4],
}
pub struct Start {
    pub address: Option<StaticAddress>,
    pub port: u16,
}
#[derive(Debug)]
pub enum LoadError {
    NoAddress,
    Busy,
    InvalidParameters,
    Bind,
}

pub mod wifi {
    pub struct Brand;
    pub const SOCKETS: usize = 2;
    pub const BLOCKS: usize = 64;
    pub const RESERVE_AHEAD: usize = oer_protocols_network_runtime::RESERVE_AHEAD;
    pub type Socket =
        oer_protocols_network_runtime::Socket<'static, 'static, Brand, 1472, 32, 32, 32>;
    pub type NetworkBlock = oer_protocols_network_core::Block<'static, Brand, 1472>;
    pub type Received = oer_protocols_network_core::Datagram<'static, Brand, 1472>;
    pub use oer_protocols_network_core::Remote;
    pub use oer_protocols_network_runtime::{Refusal, Refused};
    pub struct NetworkAddress {
        pub address: [u8; 4],
        pub prefix: u8,
        pub gateway: Option<[u8; 4]>,
    }
    pub struct Network {
        pub sockets: Option<[Socket; 2]>,
    }
    impl Network {
        pub fn configure(&mut self, _: NetworkAddress, _: [u16; 2]) -> Result<[Socket; 2], ()> {
            self.sockets.take().ok_or(())
        }
    }
}

#[path = "../src/udp.rs"]
mod udp;

#[path = "../../wifi-station/src/load/pace.rs"]
mod pace;

const LOCAL: [u8; 6] = [2; 6];
const PEER: [u8; 6] = [4; 6];
const OURS: [u8; 4] = [192, 0, 2, 1];
const THEIRS: [u8; 4] = [192, 0, 2, 2];
/// The paced level the stand measured: 100 Mbit/s of 1400-octet payloads.
const RATE_BPS: u32 = 100_000_000;
const PAYLOAD: u16 = 1400;
/// The window, in microseconds.
const WINDOW_US: u64 = 50_000;

/// The wire: it takes every frame, and the peer answers ARP.
#[derive(Default)]
struct Wire {
    incoming: VecDeque<Vec<u8>>,
    datagrams: usize,
    waker: Option<Waker>,
}

#[derive(Clone, Default)]
struct Port(Rc<RefCell<Wire>>);

impl FramePort for Port {
    type Refusal = ();
    type Ticket = ();

    fn publish<E>(
        &mut self,
        length: usize,
        fill: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<(), Unpublished<(), E>> {
        let mut frame = vec![0; length];
        fill(&mut frame).map_err(Unpublished::Fill)?;
        let mut wire = self.0.borrow_mut();
        match answer_of(&frame) {
            Some(reply) => {
                wire.incoming.push_back(reply);
                if let Some(waker) = wire.waker.take() {
                    waker.wake();
                }
            }
            None => wire.datagrams += 1,
        }
        Ok(())
    }
}

impl FrameSource for Port {
    type Notice = ();

    fn take_frame(&mut self, read: impl FnOnce(EthernetFrameView<'_>)) -> bool {
        let Some(frame) = self.0.borrow_mut().incoming.pop_front() else {
            return false;
        };
        let (header, payload) = EthernetHeader::parse(&frame).unwrap();
        read(EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        });
        true
    }

    fn take_notice(&mut self) -> Option<()> {
        None
    }

    fn register(&mut self, cx: &mut Context<'_>) {
        let mut wire = self.0.borrow_mut();
        wire.waker = Some(cx.waker().clone());
        if !wire.incoming.is_empty() {
            cx.waker().wake_by_ref();
        }
    }
}

/// The peer's ARP reply to `frame`, if it asks.
fn answer_of(frame: &[u8]) -> Option<Vec<u8>> {
    let (header, payload) = EthernetHeader::parse(frame).ok()?;
    if header.ether_type != EtherType::ARP {
        return None;
    }
    let request = Arp::parse(payload).ok()??;
    (request.operation == Operation::Request).then(|| {
        let mut reply = EthernetHeader {
            destination: LOCAL,
            source: PEER,
            ether_type: EtherType::ARP,
        }
        .to_bytes()
        .to_vec();
        let mut packet = [0; arp::PACKET_LEN];
        Arp {
            operation: Operation::Reply,
            sender_hardware: PEER,
            sender_protocol: THEIRS,
            target_hardware: LOCAL,
            target_protocol: OURS,
        }
        .write(&mut packet)
        .unwrap();
        reply.extend_from_slice(&packet);
        reply
    })
}

/// The requests the service took: reservations and sends.
#[derive(Default)]
struct Messages {
    reserves: usize,
    sends: usize,
}

impl EventSink<ServiceEvent> for Messages {
    fn event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Took(RequestKind::Reserve { .. }) => self.reserves += 1,
            ServiceEvent::Took(RequestKind::Send) => self.sends += 1,
            _ => {}
        }
    }
}

impl EventSink<NetEvent> for Messages {
    fn event(&mut self, _: NetEvent) {}
}

struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Run two tasks as an executor would: each is polled once woken, and with
/// neither woken, time moves on to the next deadline.
fn run_tasks<T, U>(
    clock: &VirtualClock,
    first: impl Future<Output = T>,
    second: impl Future<Output = U>,
) -> (T, U) {
    let mut first = pin!(first);
    let mut second = pin!(second);
    let (mut first_end, mut second_end) = (None, None);
    let flags = [Woken(AtomicBool::new(true)), Woken(AtomicBool::new(true))].map(Arc::new);
    let wakers = flags.clone().map(Waker::from);
    for _ in 0..1_000_000 {
        if first_end.is_none() && flags[0].0.swap(false, Ordering::SeqCst) {
            if let Poll::Ready(end) = first.as_mut().poll(&mut Context::from_waker(&wakers[0])) {
                first_end = Some(end);
            }
        }
        if second_end.is_none() && flags[1].0.swap(false, Ordering::SeqCst) {
            if let Poll::Ready(end) = second.as_mut().poll(&mut Context::from_waker(&wakers[1])) {
                second_end = Some(end);
            }
        }
        if let (Some(_), Some(_)) = (&first_end, &second_end) {
            return (first_end.unwrap(), second_end.unwrap());
        }
        let woken = (first_end.is_none() && flags[0].0.load(Ordering::SeqCst))
            || (second_end.is_none() && flags[1].0.load(Ordering::SeqCst));
        if !woken {
            clock
                .advance_to_next_deadline()
                .expect("no task is woken and no time is due: a wake was lost");
        }
    }
    panic!("a task spins");
}

/// What the paced sender saw.
#[derive(Debug, Default)]
struct Sent {
    taken: u64,
    /// `try_send` found no room, by the schedule's microsecond.
    refused_at: Vec<u64>,
    missed: u64,
}

#[test]
fn at_tx_100_the_image_s_paced_sender_never_waits_for_a_block_after_its_first() {
    let memory = Box::leak(Box::new([[0; 1472]; 64]));
    let (pool, brand) = claim::<wifi::Brand, 1472, 64>(memory).unwrap();
    let interface = Interface::new(LOCAL, OURS, 24, None, 1500).unwrap();
    let port = Port::default();
    let owner = NetworkOwner::new(brand, interface, pool, port.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: Service<
        'static,
        'static,
        wifi::Brand,
        1472,
        2,
        32,
        8,
        4,
        64,
        32,
        32,
        Port,
        Direct,
    > = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let receiving = service
        .bind(
            5001,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    let sending = service
        .bind(
            5002,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    let net = udp::Network::new(wifi::Network {
        sockets: Some([receiving, sending]),
    });
    let start = Start {
        address: Some(StaticAddress {
            address: OURS,
            prefix: 24,
            gateway: THEIRS,
        }),
        port: 5001,
    };
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let now_us = || clock.now().as_micros();
    let peer = udp::Peer {
        address: THEIRS,
        port: 5001,
    };
    let done = Cell::new(false);
    let mut messages = Messages::default();
    let drive = async {
        let mut run = udp::open(&net, &start).unwrap();
        let (receiving, mut sending) = run.split();
        let begin = now_us();
        let mut schedule =
            pace::Schedule::new(begin, begin + WINDOW_US, PAYLOAD, RATE_BPS, 1).unwrap();
        let mut sent = Sent::default();
        let size = usize::from(PAYLOAD);
        // As `load::serve::send` paces, with the clock read once a batch.
        loop {
            let now = now_us();
            let next = schedule.batch(now, || match sending.try_send(size, peer, |_| Some(())) {
                udp::Attempt::Taken => {
                    sent.taken += 1;
                    true
                }
                udp::Attempt::NoRoom => {
                    sent.refused_at.push(now - begin);
                    false
                }
                udp::Attempt::Lost => panic!("a datagram was lost"),
            });
            match next {
                pace::Next::Yield => yield_now().await,
                pace::Next::Wait(at) => clock.wait_until(Instant::from_micros(at)).await,
                pace::Next::Blocked => {
                    assert!(sending.send(size, peer, |_| Some(())).await);
                    sent.taken += 1;
                    schedule.taken(now_us());
                }
                pace::Next::Done => break,
            }
        }
        sent.missed = schedule.missed();
        drop((receiving, sending));
        drop(run);
        while port.0.borrow().datagrams < usize::try_from(sent.taken).unwrap() {
            yield_now().await;
        }
        done.set(true);
        stop.try_send(()).ok().unwrap();
        sent
    };
    let serve = async {
        let _cause = service.run(clock, &mut messages).await;
    };
    let ((), sent) = run_tasks(clock, serve, drive);
    assert!(done.get());
    let per_datagram = (messages.reserves + messages.sends) as f64 / messages.sends as f64;
    eprintln!(
        "paced through the image: {} taken, {} missed, refused at {:?}; {} reservations, {:.3} messages per datagram",
        sent.taken, sent.missed, sent.refused_at, messages.reserves, per_datagram
    );
    // Only the first send, before any request, finds no block.
    assert_eq!(sent.refused_at, [0], "a batch waited for its blocks");
    // What the window's last sleep, of at least the schedule's granularity,
    // leaves unsent at its end; nothing for a wait on a block.
    let last_sleep = (pace::GRANULARITY_US * u64::from(RATE_BPS) + u64::from(PAYLOAD) * 8_000_000
        - 1)
        / (u64::from(PAYLOAD) * 8_000_000);
    assert!(sent.missed <= last_sleep, "{} missed", sent.missed);
    assert_eq!(messages.sends as u64, sent.taken);
    assert!(per_datagram < 1.13, "{per_datagram}");
}
