#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]
//! Only externally published events and the bridge's bounded continuation may
//! poll this task. Virtual time never supplies a bridge wake.
use core::{future::Future, task::Poll};
use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources, ethernet::EthernetBridge};
use oer_memory::HostContext;
use oer_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, LinkSnapshot, LinkState, NetDevice,
    TransmitError, TransmitRefused,
};
use oer_time::Duration;
use oer_time_virtual::VirtualClock;
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Wake, Waker},
};
use xarxa_driver::{PacketBufAllocator, PacketPool, PacketPoolStorage};

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
fn settle(f: &mut core::pin::Pin<&mut impl Future>, scheduled: &Scheduled, cx: &mut Context<'_>) {
    for _ in 0..32 {
        if !scheduled.0.swap(false, Ordering::Relaxed) {
            return;
        }
        assert!(f.as_mut().poll(cx).is_pending());
    }
    assert!(
        !scheduled.0.load(Ordering::Relaxed),
        "bridge did not park after bounded work"
    );
}
fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}
struct Frame(u8);
impl oer_network_port::ReceivedFrame for Frame {
    type Storage = core::convert::Infallible;
    type Copied = Self;
    fn into_lent(self) -> Result<oer_network_port::LentFrame<Self::Storage>, Self> {
        Err(self)
    }
}
impl EthernetFrame for Frame {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Ok(EthernetFrameView {
            destination: [2; 6],
            source: [4; 6],
            ether_type: 0x88b5,
            payload: core::slice::from_ref(&self.0),
        })
    }
}
struct State {
    link: LinkSnapshot,
    link_waker: Option<Waker>,
    rx_waker: Option<Waker>,
    tx_waker: Option<Waker>,
    rx: VecDeque<Frame>,
    tx: Vec<u8>,
    ready: bool,
    refuse_once: bool,
}
impl State {
    fn wake(waker: &Option<Waker>) {
        if let Some(waker) = waker {
            waker.wake_by_ref();
        }
    }
    fn link(&mut self, state: LinkState, epoch: u64) {
        self.link = LinkSnapshot { state, epoch };
        Self::wake(&self.link_waker);
    }
    fn receive(&mut self, marker: u8) {
        self.rx.push_back(Frame(marker));
        Self::wake(&self.rx_waker);
    }
    fn credit(&mut self) {
        self.ready = true;
        Self::wake(&self.tx_waker);
    }
}
struct Port(Rc<RefCell<State>>, LinkSnapshot);
impl NetDevice for Port {
    type RxFrame = Frame;
    fn link(&self) -> LinkSnapshot {
        self.0.borrow().link
    }
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        let mut state = self.0.borrow_mut();
        state.link_waker = Some(cx.waker().clone());
        if state.link == self.1 {
            Poll::Pending
        } else {
            self.1 = state.link;
            Poll::Ready(self.1)
        }
    }
    fn receive_queue_len(&self) -> usize {
        self.0.borrow().rx.len()
    }
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
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
    fn poll_transmit_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
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
        let marker = frame
            .view()
            .ok()
            .and_then(|view| view.payload.first().copied());
        assert!(marker.is_some(), "test frames carry a marker");
        state.tx.extend(marker);
        Ok(())
    }
}

#[test]
fn hil_bridge_parks_retains_credit_and_services_rx_until_tx_credit_event() {
    let clock = VirtualClock::<1>::new();
    let pool = allocator::<1>();
    let mut resources = OwnedEndpointResources::<HostContext, 2, 1>::new();
    let (mut stack, radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], allocator::<2>())
        .unwrap();
    let state = Rc::new(RefCell::new(State {
        link: LinkSnapshot::DOWN,
        link_waker: None,
        rx_waker: None,
        tx_waker: None,
        rx: VecDeque::new(),
        tx: Vec::new(),
        ready: true,
        refuse_once: true,
    }));
    let mut bridge = EthernetBridge::new(
        radio,
        Port(state.clone(), LinkSnapshot::DOWN),
        None::<&'static oer_embassy_net::ethernet::LentSlots<_, 0>>,
    );
    let mut task = core::pin::pin!(core::future::poll_fn(|cx| bridge.poll(
        cx,
        |_| {},
        |_| panic!("RX refused"),
        |_| {},
        |_| {}
    )));
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = Context::from_waker(&waker);
    settle(&mut task, &scheduled, &mut cx);
    state.borrow_mut().link(LinkState::Up, 1);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.fill(7);
    stack.transmit(packet).unwrap();
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert!(!stack.can_transmit(), "refusal retains software credit");
    state.borrow_mut().receive(9);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(
        stack.receive().unwrap()[14],
        9,
        "blocked TX must not block RX"
    );
    assert!(state.borrow().tx.is_empty());
    assert_eq!(clock.next_deadline(), None);
    clock.advance(Duration::from_secs(60)).unwrap();
    assert!(!scheduled.0.load(Ordering::Relaxed));
    state.borrow_mut().credit();
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(state.borrow().tx, [7]);
    assert!(stack.can_transmit());
    // A consumed coalesced hint must have rearmed a later sparse publication.
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.fill(8);
    stack.transmit(packet).unwrap();
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(state.borrow().tx, [7, 8]);
    // Retain another owner, then change epoch while TX remains unavailable.
    state.borrow_mut().ready = false;
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.fill(10);
    stack.transmit(packet).unwrap();
    settle(&mut task, &scheduled, &mut cx);
    assert!(!stack.can_transmit());
    state.borrow_mut().link(LinkState::Down, 2);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert!(pool.has_available());
    assert!(!stack.link_is_up());
    // A down/up cycle coalesces to an Up level with a different epoch.
    state.borrow_mut().link(LinkState::Up, 3);
    settle(&mut task, &scheduled, &mut cx);
    state.borrow_mut().credit();
    settle(&mut task, &scheduled, &mut cx);
    assert!(stack.can_transmit());
    assert_eq!(
        state.borrow().tx,
        [7, 8],
        "stale owner never enters a new epoch"
    );
    // Reassociation can coalesce completely between bridge polls. An Up ->
    // Up epoch change must retire both a retained and a still-queued owner.
    for retained in [false, true] {
        state.borrow_mut().ready = false;
        let mut packet = pool.try_alloc().unwrap();
        packet.set_len(15);
        packet.fill(11);
        stack.transmit(packet).unwrap();
        if retained {
            settle(&mut task, &scheduled, &mut cx);
        }
        let epoch = state.borrow().link.epoch;
        state.borrow_mut().link(LinkState::Down, epoch);
        state.borrow_mut().link(LinkState::Up, epoch + 1);
        settle(&mut task, &scheduled, &mut cx);
        assert!(stack.link_is_up());
        assert!(stack.can_transmit());
        assert!(pool.has_available());
    }
}
