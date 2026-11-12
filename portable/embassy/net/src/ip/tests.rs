extern crate std;

use super::*;
use crate::sink::Sink;
use core::{cell::RefCell, pin::Pin, task::Waker};
use oer_portable_memory::HostContext;
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_port::{EthernetFrame, FrameLengthError};
use std::{
    boxed::Box,
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Wake,
    vec::Vec,
};

const CONFIG: Ipv4Configuration = Ipv4Configuration {
    address: [192, 0, 2, 2],
    prefix_length: 24,
    gateway: Some([192, 0, 2, 1]),
};

#[derive(Default)]
struct Scheduled(AtomicBool);
impl Wake for Scheduled {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }
}
fn settle<F: Future>(
    task: &mut Pin<&mut F>,
    scheduled: &Scheduled,
    cx: &mut core::task::Context<'_>,
) {
    for _ in 0..64 {
        if !scheduled.0.swap(false, Ordering::Relaxed) {
            return;
        }
        assert!(task.as_mut().poll(cx).is_pending());
    }
    assert!(
        !scheduled.0.load(Ordering::Relaxed),
        "service must park after bounded work"
    );
}

/// A device frame: the bridge copies a plain one; a lent one stays in its
/// DMA-like storage.
enum Frame {
    Plain,
    Lent(oer_protocols_network_port::LentFrame<Lent>),
}
impl EthernetFrame for Frame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        match self {
            Self::Plain => Ok(EthernetFrameView {
                destination: [2; 6],
                source: [4; 6],
                ether_type: 0x88b5,
                payload: &[9],
            }),
            Self::Lent(frame) => frame.view(),
        }
    }
}
/// A lent receive buffer and the count of its returns.
struct Lent {
    bytes: Box<[u8; 1664]>,
    start: usize,
    returned: Arc<std::sync::atomic::AtomicUsize>,
}
impl oer_protocols_network_port::FrameStorage for Lent {
    fn bytes(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..]
    }
    fn advance(
        &mut self,
        by: usize,
    ) -> Result<(), oer_protocols_network_port::StorageAdvanceRefused> {
        let length = self.bytes.len() - self.start;
        if by > length {
            return Err(oer_protocols_network_port::StorageAdvanceRefused { by, length });
        }
        self.start += by;
        Ok(())
    }
}
impl Drop for Lent {
    fn drop(&mut self) {
        self.returned.fetch_add(1, Ordering::Relaxed);
    }
}
/// A lent frame from `[4; 6]` to the device, its payload 60 bytes into its
/// buffer as after a Wi-Fi header.
fn lent(ether_type: u16, payload: &[u8], returned: &Arc<std::sync::atomic::AtomicUsize>) -> Frame {
    let mut bytes = Box::new([0xee; 1664]);
    bytes[60..60 + payload.len()].copy_from_slice(payload);
    let storage = Lent {
        bytes,
        start: 0,
        returned: returned.clone(),
    };
    let Ok(frame) = oer_protocols_network_port::LentFrame::new(
        storage,
        [2; 6],
        [4; 6],
        ether_type,
        60..60 + payload.len(),
    ) else {
        panic!("the payload lies in the buffer");
    };
    Frame::Lent(frame)
}
/// The ones' complement sum of `bytes`, as IPv4 and ICMP checksums.
fn checksum(bytes: &[u8]) -> [u8; 2] {
    let mut sum = 0_u32;
    for pair in bytes.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    (!(sum as u16)).to_be_bytes()
}
const PEER: [u8; 4] = [192, 0, 2, 9];
/// An ARP request from the peer for the device's address.
fn arp_request() -> Vec<u8> {
    let mut arp = std::vec![0, 1, 8, 0, 6, 4, 0, 1];
    arp.extend_from_slice(&[4; 6]);
    arp.extend_from_slice(&PEER);
    arp.extend_from_slice(&[0; 6]);
    arp.extend_from_slice(&CONFIG.address);
    arp
}
/// An IPv4 ICMP echo request from the peer to the device.
fn echo_request() -> Vec<u8> {
    let mut icmp = std::vec![8, 0, 0, 0, 0, 1, 0, 1, 1, 2, 3, 4];
    let sum = checksum(&icmp);
    icmp[2..4].copy_from_slice(&sum);
    let mut ip = std::vec![0x45, 0, 0, (20 + icmp.len()) as u8, 0, 1, 0, 0, 64, 1, 0, 0];
    ip.extend_from_slice(&PEER);
    ip.extend_from_slice(&CONFIG.address);
    let sum = checksum(&ip);
    ip[10..12].copy_from_slice(&sum);
    ip.extend_from_slice(&icmp);
    ip
}
struct PortState {
    link: LinkSnapshot,
    link_waker: Option<Waker>,
    rx_waker: Option<Waker>,
    rx: VecDeque<Frame>,
    /// The fault the queue meets after its frames, once.
    fault: Option<oer_protocols_network_port::ReceiveFault>,
    /// Receive calls after the fault.
    after_fault: usize,
}
impl PortState {
    fn wake(waker: &Option<Waker>) {
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }
    fn link(&mut self, state: LinkState, epoch: u64) {
        self.link = LinkSnapshot { state, epoch };
        Self::wake(&self.link_waker);
    }
}
struct IpPort(Rc<RefCell<PortState>>);
impl NetDevice for IpPort {
    type Storage = Lent;
    fn link(&self) -> LinkSnapshot {
        self.0.borrow().link
    }
    fn poll_link(&mut self, cx: &mut core::task::Context<'_>) -> Poll<LinkSnapshot> {
        let mut state = self.0.borrow_mut();
        state.link_waker = Some(cx.waker().clone());
        Poll::Ready(state.link)
    }
    fn receive_queue_len(&self) -> usize {
        self.0.borrow().rx.len()
    }
    fn poll_receive_ready(&mut self, cx: &mut core::task::Context<'_>) -> Poll<()> {
        let mut state = self.0.borrow_mut();
        state.rx_waker = Some(cx.waker().clone());
        if state.rx.is_empty() && state.fault.is_none() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(
            oer_protocols_network_port::Incoming<'_, oer_protocols_network_port::LentFrame<Lent>>,
        ),
    ) -> oer_protocols_network_port::ReceivedBatch {
        use oer_protocols_network_port::{BatchEnd, CopyReason, Incoming, ReceivedBatch};
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let next = {
                let mut state = self.0.borrow_mut();
                if state.after_fault > 0 {
                    state.after_fault += 1;
                }
                match state.rx.pop_front() {
                    Some(frame) => Ok(frame),
                    None => Err(state.fault.take()),
                }
            };
            let frame = match next {
                Ok(frame) => frame,
                Err(Some(fault)) => {
                    self.0.borrow_mut().after_fault = 1;
                    break BatchEnd::Fault(fault);
                }
                Err(None) => break BatchEnd::Drained,
            };
            taken += 1;
            match frame {
                Frame::Lent(frame) => take(Incoming::Lent(frame)),
                mut plain @ Frame::Plain => take(Incoming::Copied {
                    frame: plain.view(),
                    reason: CopyReason::Format,
                }),
            }
        };
        ReceivedBatch { taken, end }
    }
}
fn port() -> (IpPort, Rc<RefCell<PortState>>) {
    let state = Rc::new(RefCell::new(PortState {
        link: LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        },
        link_waker: None,
        rx_waker: None,
        rx: VecDeque::new(),
        fault: None,
        after_fault: 0,
    }));
    (IpPort(state.clone()), state)
}
/// The lent packet slots of the services these tests start.
const LENT: usize = 4;

fn start<X: ExecutionContext>(
    configuration: Configuration,
) -> (
    Control<'static>,
    Service<X, 2, Sink, IpPort, LENT>,
    Rc<RefCell<PortState>>,
    Sink,
) {
    start_lent(configuration, None)
}

fn start_lent<X: ExecutionContext>(
    configuration: Configuration,
    lent: Option<&'static crate::ethernet::LentSlots<Lent, LENT>>,
) -> (
    Control<'static>,
    Service<X, 2, Sink, IpPort, LENT>,
    Rc<RefCell<PortState>>,
    Sink,
) {
    let (port, state) = port();
    let sink = Sink::with_room(4);
    let resources = Box::leak(Box::new(Resources::<X, 2, Sink>::new()));
    let (control, service) = resources
        .start(
            port,
            sink.clone(),
            lent,
            general::<8>(),
            NetworkInterfaceId::new(0),
            [2; 6],
            1,
            configuration,
        )
        .unwrap_or_else(|reason| panic!("start refused: {reason:?}"));
    (control, service, state, sink)
}

// The stack boundary supplies lease events and protocol deadlines. Production
// uses the pinned Iface and Runner; these tests exercise the same service loop
// with a per-test clock rather than Embassy's process-global time ABI.
struct StackEvents<'a> {
    clock: &'a VirtualClock,
    lease: Cell<Option<Ipv4Configuration>>,
    pending: Cell<Option<Option<Ipv4Configuration>>>,
    expiry: Cell<Option<Instant>>,
    waker: Cell<Option<Waker>>,
    resets: Cell<u32>,
}
impl<'a> StackEvents<'a> {
    fn new(clock: &'a VirtualClock) -> Self {
        Self {
            clock,
            lease: Cell::new(None),
            pending: Cell::new(None),
            expiry: Cell::new(None),
            waker: Cell::new(None),
            resets: Cell::new(0),
        }
    }
    fn offer(&self, config: Ipv4Configuration) {
        self.pending.set(Some(Some(config)));
        self.expiry
            .set(self.clock.now().checked_add(Duration::from_secs(60)));
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
    fn poll(&self, cx: &mut core::task::Context<'_>) {
        self.waker.set(Some(cx.waker().clone()));
        if let Some(lease) = self.pending.take() {
            self.lease.set(lease);
        }
        if let Some(expiry) = self.expiry.get() {
            let mut deadline = core::pin::pin!(self.clock.wait_until(expiry));
            if deadline.as_mut().poll(cx).is_ready() {
                self.lease.set(None);
                self.expiry.set(None);
            }
        }
    }
}
impl Interface for StackEvents<'_> {
    fn restart_dhcp(&self) {
        self.resets.set(self.resets.get().saturating_add(1));
        self.lease.set(None);
        self.pending.set(None);
        self.expiry.set(None);
    }
    fn lease(&self) -> Option<Ipv4Configuration> {
        self.lease.get()
    }
    fn clear(&self) -> Result<(), Error> {
        self.restart_dhcp();
        Ok(())
    }
}

#[test]
fn lease_events_expiry_and_coalesced_reconnect_keep_the_stack_and_socket() {
    let clock = VirtualClock::new();
    let stack_events = StackEvents::new(&clock);
    let (control, mut service, port, _) = start::<HostContext>(Configuration::Dhcpv4);
    let stack = service.stack();
    let iface = service.interface();
    let marker = Ipv4Addr::new(198, 51, 100, 8);
    iface
        .add_ip_addr(IpCidr::V4(Ipv4Cidr::new(marker, 24)))
        .unwrap();
    let mut socket = owned_embassy_net::udp::UdpSocket::new(stack).unwrap();
    socket
        .bind(1234, owned_embassy_net::wire::ListenSocketAddr::UNSPECIFIED)
        .unwrap();
    let events = RefCell::new(Vec::new());
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    {
        let mut task = core::pin::pin!(drive(
            service.state,
            &mut service.bridge,
            &stack_events,
            Configuration::Dhcpv4,
            |cx| stack_events.poll(cx),
            |event| events.borrow_mut().push(event),
            |_| {},
            |_| {},
            |_| {},
        ));
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(control.status().link.epoch, 1);
        assert_eq!(control.status().ipv4, Ipv4Status::Unconfigured);
        stack_events.offer(CONFIG);
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(control.status().ipv4, Ipv4Status::Lease(CONFIG));
        clock.advance(Duration::from_secs(60)).unwrap();
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(
            control.status().ipv4,
            Ipv4Status::Unconfigured,
            "protocol expiry is observable without a sampler"
        );
        stack_events.offer(CONFIG);
        settle(&mut task, &scheduled, &mut cx);
        port.borrow_mut().link(LinkState::Down, 1);
        port.borrow_mut().link(LinkState::Up, 2);
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(control.status().link.epoch, 2);
        assert_eq!(
            control.status().ipv4,
            Ipv4Status::Unconfigured,
            "the old lease cannot cross an epoch"
        );
        assert!(
            iface.has_ip_addr(marker),
            "reconnect preserves existing stack state"
        );
        assert_eq!(stack.ifaces().count(), 1);
        let renewed = Ipv4Configuration {
            gateway: None,
            ..CONFIG
        };
        stack_events.offer(renewed);
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(control.status().ipv4, Ipv4Status::Lease(renewed));
        stack_events.offer(CONFIG);
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(
            control.status().ipv4,
            Ipv4Status::Lease(CONFIG),
            "configured renewals publish changed values"
        );
        let before = events.borrow().len();
        scheduled.0.store(true, Ordering::Relaxed);
        settle(&mut task, &scheduled, &mut cx);
        assert_eq!(
            events.borrow().len(),
            before,
            "no duplicate status on unrelated polls"
        );
        control.stop();
        assert!(scheduled.0.swap(false, Ordering::Relaxed));
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!(control.status().lifecycle, Lifecycle::Stopped);
    assert_eq!(control.status().ipv4, Ipv4Status::Unconfigured);
    assert!(
        events
            .borrow()
            .iter()
            .any(|event| event.previous.link.epoch == 1
                && event.current.link.epoch == 2
                && event.current.ipv4 == Ipv4Status::Unconfigured)
    );
    assert_eq!(service.interface().handle(), iface.handle());
    drop(socket);
}

#[test]
fn a_full_device_stops_no_reception_and_only_its_room_or_a_frame_wakes_the_service() {
    let clock = VirtualClock::new();
    let stack_events = StackEvents::new(&clock);
    let (control, mut service, port, sink) = start::<HostContext>(Configuration::Dhcpv4);
    let iface = service.interface();
    let received = Cell::new(0u32);
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut task = core::pin::pin!(drive(
        service.state,
        &mut service.bridge,
        &stack_events,
        Configuration::Dhcpv4,
        |cx| stack_events.poll(cx),
        |_| {},
        |_| received.set(received.get().saturating_add(1)),
        |_| {},
        |_| {},
    ));
    settle(&mut task, &scheduled, &mut cx);
    sink.0.borrow_mut().room = 0;
    assert!(!iface.with_driver(|driver| driver.can_transmit()));
    port.borrow_mut().rx.push_back(Frame::Plain);
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(received.get(), 1);
    assert_eq!(clock.next_deadline(), None);
    clock.advance(Duration::from_secs(600)).unwrap();
    assert!(
        !scheduled.0.load(Ordering::Relaxed),
        "virtual time alone cannot wake an idle/blocked service"
    );
    // A frame the device still queues at the stop may be lent: the stop
    // drops it, which returns it to the device.
    port.borrow_mut().rx.push_back(Frame::Plain);
    control.stop();
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert!(port.borrow().rx.is_empty());
}

#[test]
fn public_stop_clears_static_configuration() {
    let (control, mut service, _, _) = start::<HostContext>(Configuration::Static(CONFIG));
    let iface = service.interface();
    // Prime the bridge without polling Embassy's time ABI.
    let waker = Waker::noop();
    let mut cx = core::task::Context::from_waker(waker);
    let _ = service.bridge.poll(&mut cx, |_| {}, |_| {}, |_| {});
    assert!(iface.has_ip_addr(Ipv4Addr::from(CONFIG.address)));
    assert_eq!(service.stack().routes().len(), 1);
    control.stop();
    control.stop();
    {
        let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert!(!iface.is_link_up());
    assert!(!iface.is_config_v4_up());
    assert!(service.stack().routes().is_empty());
    assert_eq!(control.status().lifecycle, Lifecycle::Stopped);
    // A terminal service can be polled again without touching the stack ABI.
    let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
}

#[test]
fn static_status_tracks_link_events_and_stop_clears_the_real_interface() {
    let clock = VirtualClock::<1>::new();
    let (control, mut service, port, _) = start::<HostContext>(Configuration::Static(CONFIG));
    let iface = service.interface();
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut task = core::pin::pin!(drive(
        service.state,
        &mut service.bridge,
        &iface,
        Configuration::Static(CONFIG),
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        |_| {},
    ));
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(control.status().ipv4, Ipv4Status::Static(CONFIG));
    clock.advance(Duration::from_secs(100)).unwrap();
    assert!(!scheduled.0.load(Ordering::Relaxed));
    port.borrow_mut().link(LinkState::Down, 1);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(control.status().ipv4, Ipv4Status::Unconfigured);
    port.borrow_mut().link(LinkState::Up, 2);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(control.status().ipv4, Ipv4Status::Static(CONFIG));
    assert_eq!(control.status().link.epoch, 2);
    assert!(iface.has_ip_addr(Ipv4Addr::from(CONFIG.address)));
    assert_eq!(iface.stack().routes().len(), 1);
    control.stop();
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert!(!iface.is_config_up());
    assert!(iface.stack().routes().is_empty());
}

#[test]
fn invalid_static_configuration_is_refused_by_name() {
    for (config, expected) in [
        (
            Ipv4Configuration {
                prefix_length: 33,
                ..CONFIG
            },
            Error::InvalidPrefixLength(33),
        ),
        (
            Ipv4Configuration {
                address: [0; 4],
                ..CONFIG
            },
            Error::AddressNotUnicast,
        ),
        (
            Ipv4Configuration {
                gateway: Some([255; 4]),
                ..CONFIG
            },
            Error::GatewayNotUnicast,
        ),
    ] {
        let (port, _) = port();
        let resources = Box::leak(Box::new(Resources::<HostContext, 1, Sink>::new()));
        let refused = resources
            .start(
                port,
                Sink::with_room(1),
                None::<&'static crate::ethernet::LentSlots<Lent, LENT>>,
                general::<1>(),
                NetworkInterfaceId::new(0),
                [2; 6],
                1,
                Configuration::Static(config),
            )
            .err()
            .unwrap();
        assert_eq!(refused, expected);
    }
}

/// A general pool of `N` packets, as the stack's general allocator.
fn general<const N: usize>() -> xarxa_driver::PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

#[test]
fn a_general_allocator_another_stack_waits_on_is_refused_before_a_stack_is_built() {
    let (port, _) = port();
    let general = general::<1>();
    let _held = general.try_claim_waiter().unwrap();
    let resources = Box::leak(Box::new(Resources::<HostContext, 1, Sink>::new()));
    let refused = resources
        .start(
            port,
            Sink::with_room(1),
            None::<&'static crate::ethernet::LentSlots<Lent, LENT>>,
            general,
            NetworkInterfaceId::new(0),
            [2; 6],
            1,
            Configuration::Dhcpv4,
        )
        .err()
        .unwrap();
    assert_eq!(refused, Error::GeneralWaiterUnavailable);
}

#[test]
fn a_stop_returns_every_lent_buffer_including_the_replies_the_stack_retains() {
    let slots: &'static crate::ethernet::LentSlots<Lent, 4> =
        Box::leak(Box::new(crate::ethernet::LentSlots::new()));
    let (control, mut service, port, sink) =
        start_lent::<HostContext>(Configuration::Static(CONFIG), Some(slots));
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
    settle(&mut task, &scheduled, &mut cx);
    // The device has no room: the stack's replies wait inside it, each in
    // its request's packet.
    sink.0.borrow_mut().room = 0;
    let returned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // The peer's ARP request teaches the stack its MAC; its buffer returns
    // once the stack has read it.
    port.borrow_mut()
        .rx
        .push_back(lent(0x0806, &arp_request(), &returned));
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(returned.load(Ordering::Relaxed), 1);
    // The echo reply reuses its request's lent buffer and waits for the
    // device's room inside the stack.
    port.borrow_mut()
        .rx
        .push_back(lent(0x0800, &echo_request(), &returned));
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert!(port.borrow().rx.is_empty(), "the bridge took both frames");
    assert_eq!(
        returned.load(Ordering::Relaxed),
        1,
        "the retained echo reply holds its lent buffer"
    );
    control.stop();
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(
        returned.load(Ordering::Relaxed),
        2,
        "the stop returns the retained reply's lent buffer"
    );
}

/// A UDP datagram from the peer to `port` of the static address.
fn udp_datagram(port: u16, payload: &[u8]) -> Vec<u8> {
    let length = 8 + payload.len();
    let mut ip = std::vec![0x45, 0, 0, (20 + length) as u8, 0, 1, 0, 0, 64, 17, 0, 0];
    ip.extend_from_slice(&PEER);
    ip.extend_from_slice(&CONFIG.address);
    let sum = checksum(&ip);
    ip[10..12].copy_from_slice(&sum);
    // The UDP checksum is optional over IPv4: zero carries none.
    ip.extend_from_slice(&4321_u16.to_be_bytes());
    ip.extend_from_slice(&port.to_be_bytes());
    ip.extend_from_slice(&(length as u16).to_be_bytes());
    ip.extend_from_slice(&[0, 0]);
    ip.extend_from_slice(payload);
    ip
}

#[test]
fn a_stop_returns_the_lent_buffers_an_open_socket_queues() {
    let slots: &'static crate::ethernet::LentSlots<Lent, 4> =
        Box::leak(Box::new(crate::ethernet::LentSlots::new()));
    let (control, mut service, port, _) =
        start_lent::<HostContext>(Configuration::Static(CONFIG), Some(slots));
    let mut socket = owned_embassy_net::udp::UdpSocket::new(service.stack()).unwrap();
    socket
        .bind(1234, owned_embassy_net::wire::ListenSocketAddr::UNSPECIFIED)
        .unwrap();
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}));
    settle(&mut task, &scheduled, &mut cx);
    let returned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for payload in [[1, 2, 3], [4, 5, 6]] {
        port.borrow_mut()
            .rx
            .push_back(lent(0x0800, &udp_datagram(1234, &payload), &returned));
    }
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert!(
        port.borrow().rx.is_empty(),
        "the bridge took both datagrams"
    );
    assert_eq!(
        returned.load(Ordering::Relaxed),
        0,
        "the socket queues both datagrams in their lent buffers"
    );
    control.stop();
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert_eq!(
        returned.load(Ordering::Relaxed),
        2,
        "the stop returns the lent buffers of the socket the image still holds"
    );
    assert!(!socket.is_open());
}

/// A receive queue that breaks while the stack holds lent buffers: the
/// service ends with that fault, after the cleanup a stop does (the stack's
/// packets dropped, lent buffers back, the configuration cleared), and reads
/// the broken queue no more.
#[test]
fn a_broken_receive_queue_ends_the_service_with_its_fault_after_a_full_cleanup() {
    let slots: &'static crate::ethernet::LentSlots<Lent, 4> =
        Box::leak(Box::new(crate::ethernet::LentSlots::new()));
    let (_control, mut service, port, _) =
        start_lent::<HostContext>(Configuration::Static(CONFIG), Some(slots));
    let mut socket = owned_embassy_net::udp::UdpSocket::new(service.stack()).unwrap();
    socket
        .bind(1234, owned_embassy_net::wire::ListenSocketAddr::UNSPECIFIED)
        .unwrap();
    let mut statuses = Vec::new();
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let returned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let mut task =
            core::pin::pin!(service.run(|status| statuses.push(status), |_| {}, |_| {}, |_| {}));
        settle(&mut task, &scheduled, &mut cx);
        for payload in [[1, 2, 3], [4, 5, 6]] {
            port.borrow_mut()
                .rx
                .push_back(lent(0x0800, &udp_datagram(1234, &payload), &returned));
        }
        port.borrow_mut().fault = Some(oer_protocols_network_port::ReceiveFault::Unfilled);
        PortState::wake(&port.borrow().rx_waker);
        let mut ended = None;
        for _ in 0..8 {
            if let Poll::Ready(result) = task.as_mut().poll(&mut cx) {
                ended = Some(result);
                break;
            }
        }
        assert_eq!(
            ended,
            Some(Err(Error::ReceiveFault(
                oer_protocols_network_port::ReceiveFault::Unfilled
            ))),
            "the original fault ends the service"
        );
    }
    assert_eq!(
        returned.load(Ordering::Relaxed),
        2,
        "the lent buffers the socket queued are back"
    );
    assert_eq!(
        port.borrow().after_fault,
        1,
        "the broken queue is read no more"
    );
    assert!(!socket.is_open(), "the stack's sockets are closed");
    let last = statuses.last().unwrap().current;
    assert_eq!(last.link.state, LinkState::Down);
    assert_eq!(last.ipv4, Ipv4Status::Unconfigured);
    assert_eq!(
        last.lifecycle,
        Lifecycle::Failed(Error::ReceiveFault(
            oer_protocols_network_port::ReceiveFault::Unfilled
        ))
    );
}
