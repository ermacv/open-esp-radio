extern crate std;

use super::*;
use core::{cell::RefCell, pin::Pin, task::Waker};
use oer_memory::HostContext;
use oer_network_port::{EthernetFrame, FrameLengthError, TransmitError, TransmitRefused};
use oer_time::{Clock, Duration, Instant, Timer};
use oer_time_virtual::VirtualClock;
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
use xarxa_driver::{PacketBuf, PacketBufAllocator};

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
    Lent(oer_network_port::LentFrame<Lent>),
}
impl oer_network_port::ReceivedFrame for Frame {
    type Storage = Lent;
    type Copied = Self;
    fn into_lent(self) -> Result<oer_network_port::LentFrame<Lent>, Self> {
        match self {
            Self::Lent(frame) => Ok(frame),
            plain @ Self::Plain => Err(plain),
        }
    }
}
impl EthernetFrame for Frame {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
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
impl oer_network_port::FrameStorage for Lent {
    fn bytes(&self) -> &[u8] {
        &self.bytes[self.start..]
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.start..]
    }
    fn advance(&mut self, by: usize) -> Result<(), oer_network_port::StorageAdvanceRefused> {
        let length = self.bytes.len() - self.start;
        if by > length {
            return Err(oer_network_port::StorageAdvanceRefused { by, length });
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
    let Ok(frame) = oer_network_port::LentFrame::new(
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
    tx_waker: Option<Waker>,
    rx: VecDeque<Frame>,
    accepted: Vec<usize>,
    ready: bool,
    refuse_once: bool,
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
struct Port(Rc<RefCell<PortState>>);
impl NetDevice for Port {
    type RxFrame = Frame;
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
        if state.rx.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
    fn try_receive(&mut self) -> Option<Frame> {
        self.0.borrow_mut().rx.pop_front()
    }
    fn read_copied<R>(
        &mut self,
        copied: Frame,
        read: impl FnOnce(EthernetFrameView<'_>) -> R,
    ) -> Result<R, FrameLengthError> {
        copied.view().map(read)
    }
    fn poll_transmit_ready(&mut self, cx: &mut core::task::Context<'_>) -> Poll<()> {
        let mut state = self.0.borrow_mut();
        state.tx_waker = Some(cx.waker().clone());
        if state.ready && state.link.state == LinkState::Up {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
    fn try_transmit<F: EthernetFrame>(&mut self, frame: F) -> Result<(), TransmitRefused<F>> {
        let mut state = self.0.borrow_mut();
        if !state.ready || state.refuse_once {
            state.refuse_once = false;
            state.ready = false;
            return Err(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            });
        }
        state
            .accepted
            .push(frame.view().unwrap().payload.as_ptr() as usize);
        Ok(())
    }
}
fn port() -> (Port, Rc<RefCell<PortState>>) {
    let state = Rc::new(RefCell::new(PortState {
        link: LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        },
        link_waker: None,
        rx_waker: None,
        tx_waker: None,
        rx: VecDeque::new(),
        accepted: Vec::new(),
        ready: true,
        refuse_once: false,
    }));
    (Port(state.clone()), state)
}
fn start<X: ExecutionContext>(
    configuration: Configuration,
) -> (
    Control<'static>,
    Service<X, 2, 1, Port>,
    Rc<RefCell<PortState>>,
) {
    start_lent(configuration, None)
}

fn start_lent<X: ExecutionContext>(
    configuration: Configuration,
    lent: Option<&'static dyn crate::ethernet::AdoptLent<Lent>>,
) -> (
    Control<'static>,
    Service<X, 2, 1, Port>,
    Rc<RefCell<PortState>>,
) {
    let (port, state) = port();
    let resources = Box::leak(Box::new(Resources::<X, 2, 1, 8>::new()));
    let (control, service) = resources
        .start(
            port,
            lent,
            NetworkInterfaceId::new(0),
            [2; 6],
            1,
            configuration,
        )
        .unwrap_or_else(|refused| panic!("start refused: {:?}", refused.reason));
    (control, service, state)
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
    let (control, mut service, port) = start::<HostContext>(Configuration::Dhcpv4);
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

fn allocator() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}
fn packet(pool: PacketBufAllocator) -> PacketBuf {
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.copy_from_slice(&[2, 2, 2, 2, 2, 2, 4, 4, 4, 4, 4, 4, 0x88, 0xb5, 9]);
    packet
}

#[test]
fn tx_refusal_keeps_the_exact_owner_until_credit_and_rx_still_progresses() {
    let clock = VirtualClock::new();
    let stack_events = StackEvents::new(&clock);
    let (control, mut service, port) = start::<HostContext>(Configuration::Dhcpv4);
    let iface = service.interface();
    let pool = allocator();
    let packet = packet(pool);
    let original_payload = packet.get(14..).unwrap().as_ptr() as usize;
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
        |_| {},
    ));
    settle(&mut task, &scheduled, &mut cx);
    port.borrow_mut().refuse_once = true;
    iface.with_driver(|driver| driver.transmit(packet)).unwrap();
    settle(&mut task, &scheduled, &mut cx);
    assert!(pool.try_alloc().is_none());
    assert!(
        !iface.with_driver(|driver| driver.can_transmit()),
        "retained packet keeps the single TX credit"
    );
    port.borrow_mut().rx.push_back(Frame::Plain);
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(received.get(), 1);
    assert!(pool.try_alloc().is_none());
    assert_eq!(clock.next_deadline(), None);
    clock.advance(Duration::from_secs(600)).unwrap();
    assert!(
        !scheduled.0.load(Ordering::Relaxed),
        "virtual time alone cannot wake an idle/blocked service"
    );
    port.borrow_mut().ready = true;
    PortState::wake(&port.borrow().tx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(port.borrow().accepted, [original_payload]);
    assert!(pool.try_alloc().is_some());
    assert!(iface.with_driver(|driver| driver.can_transmit()));
    // A frame the device still queues at the stop may be lent: the stop
    // drops it, which returns it to the device.
    port.borrow_mut().rx.push_back(Frame::Plain);
    control.stop();
    assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    assert!(port.borrow().rx.is_empty());
}

#[test]
fn public_stop_reclaims_retained_and_queued_tx_and_clears_static_configuration() {
    for retained in [false, true] {
        let (control, mut service, port) = start::<HostContext>(Configuration::Static(CONFIG));
        let iface = service.interface();
        let pool = allocator();
        // Prime the bridge without polling Embassy's time ABI.
        let waker = Waker::noop();
        let mut cx = core::task::Context::from_waker(waker);
        assert!(
            service
                .bridge
                .poll(&mut cx, |_| {}, |_| {}, |_| {}, |_| {})
                .is_pending()
        );
        assert!(iface.has_ip_addr(Ipv4Addr::from(CONFIG.address)));
        assert_eq!(service.stack().routes().len(), 1);
        port.borrow_mut().ready = false;
        iface
            .with_driver(|driver| driver.transmit(packet(pool)))
            .unwrap();
        if retained {
            assert!(
                service
                    .bridge
                    .poll(&mut cx, |_| {}, |_| {}, |_| {}, |_| {})
                    .is_pending()
            );
        }
        assert!(pool.try_alloc().is_none());
        control.stop();
        control.stop();
        {
            let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}, |_| {}));
            assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        }
        assert!(pool.try_alloc().is_some());
        assert!(!iface.is_link_up());
        assert!(!iface.is_config_v4_up());
        assert!(service.stack().routes().is_empty());
        assert_eq!(control.status().lifecycle, Lifecycle::Stopped);
        // A terminal service can be polled again without touching the stack ABI.
        let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}, |_| {}));
        assert_eq!(task.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
}

#[test]
fn context_refusal_is_a_typed_terminal_and_does_not_poll_the_stack() {
    let (control, mut service, _) = start::<oer_memory::Never>(Configuration::Dhcpv4);
    let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}, |_| {}));
    assert_eq!(
        task.as_mut()
            .poll(&mut core::task::Context::from_waker(Waker::noop())),
        Poll::Ready(Err(Error::ContextAccessRefused))
    );
    assert_eq!(
        control.status().lifecycle,
        Lifecycle::Failed(Error::ContextAccessRefused)
    );
}

#[test]
fn static_status_tracks_link_events_and_stop_clears_the_real_interface() {
    let clock = VirtualClock::<1>::new();
    let (control, mut service, port) = start::<HostContext>(Configuration::Static(CONFIG));
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
fn invalid_static_configuration_returns_the_original_device() {
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
        let (port, original) = port();
        let resources = Box::leak(Box::new(Resources::<HostContext, 1, 1, 1>::new()));
        let refused = resources
            .start(
                port,
                None,
                NetworkInterfaceId::new(0),
                [2; 6],
                1,
                Configuration::Static(config),
            )
            .err()
            .unwrap();
        assert_eq!(refused.reason, expected);
        assert!(Rc::ptr_eq(&refused.device.0, &original));
    }
}

#[test]
fn a_stop_returns_every_lent_buffer_including_the_replies_the_stack_retains() {
    let slots: &'static crate::ethernet::LentSlots<Lent, 4> =
        Box::leak(Box::new(crate::ethernet::LentSlots::new()));
    let (control, mut service, port) =
        start_lent::<HostContext>(Configuration::Static(CONFIG), Some(slots));
    let iface = service.interface();
    let pool = allocator();
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = core::task::Context::from_waker(&waker);
    let mut task = core::pin::pin!(service.run(|_| {}, |_| {}, |_| {}, |_| {}, |_| {}));
    settle(&mut task, &scheduled, &mut cx);
    // The device takes no frame and the stack's one TX credit is out: its
    // replies wait inside it, each in its request's packet.
    port.borrow_mut().ready = false;
    iface
        .with_driver(|driver| driver.transmit(packet(pool)))
        .unwrap();
    settle(&mut task, &scheduled, &mut cx);
    let returned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // The peer's ARP request teaches the stack its MAC; its buffer returns
    // once the stack has read it.
    port.borrow_mut()
        .rx
        .push_back(lent(0x0806, &arp_request(), &returned));
    PortState::wake(&port.borrow().rx_waker);
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(returned.load(Ordering::Relaxed), 1);
    // The echo reply reuses its request's lent buffer and waits for TX
    // credit inside the stack.
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
