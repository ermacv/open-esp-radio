#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]
//! Only externally published events and the bridge's bounded continuation may
//! poll this task. Virtual time never supplies a bridge wake.
use core::{future::Future, task::Poll};
use oer_portable_embassy_net::{
    NetworkInterfaceId, OwnedEndpointResources, OwnedNetworkTxFrame, ethernet::EthernetBridge,
};
use oer_portable_memory::HostContext;
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, LinkSnapshot, LinkState, NetDevice,
    NetTransmit, TransmitRefused,
};
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
impl EthernetFrame for Frame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
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
    rx: VecDeque<Frame>,
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
}
struct Port(Rc<RefCell<State>>, LinkSnapshot);
/// A transmit half that takes nothing: the bridge has no part in sending.
struct NoTransmit;
impl NetTransmit for NoTransmit {
    type Frame = OwnedNetworkTxFrame;
    fn reclaim(&mut self) {}
    fn has_room(&mut self) -> bool {
        false
    }
    fn poll_room(&mut self, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
    fn flush(&mut self) {}
    fn try_transmit(
        &mut self,
        frame: OwnedNetworkTxFrame,
    ) -> Result<(), TransmitRefused<OwnedNetworkTxFrame>> {
        Err(TransmitRefused {
            frame,
            reason: oer_protocols_network_port::TransmitError::QueueFull,
        })
    }
}
impl NetDevice for Port {
    type Storage = core::convert::Infallible;
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
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(
            oer_protocols_network_port::Incoming<
                '_,
                oer_protocols_network_port::LentFrame<core::convert::Infallible>,
            >,
        ),
    ) -> oer_protocols_network_port::ReceivedBatch {
        use oer_protocols_network_port::{BatchEnd, CopyReason, Incoming, ReceivedBatch};
        let mut taken = 0;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let Some(mut frame) = self.0.borrow_mut().rx.pop_front() else {
                break BatchEnd::Drained;
            };
            taken += 1;
            take(Incoming::Copied {
                frame: frame.view(),
                reason: CopyReason::Format,
            });
        };
        ReceivedBatch { taken, end }
    }
}

#[test]
fn the_bridge_parks_and_only_a_link_change_or_a_frame_wakes_it() {
    let clock = VirtualClock::<1>::new();
    let mut resources = OwnedEndpointResources::<HostContext, 2>::new();
    let (mut stack, radio) = resources
        .split(
            NetworkInterfaceId::new(0),
            [2; 6],
            allocator::<2>(),
            NoTransmit,
        )
        .unwrap();
    let state = Rc::new(RefCell::new(State {
        link: LinkSnapshot::DOWN,
        link_waker: None,
        rx_waker: None,
        rx: VecDeque::new(),
    }));
    let mut bridge = EthernetBridge::new(
        radio,
        Port(state.clone(), LinkSnapshot::DOWN),
        None::<&'static oer_portable_embassy_net::ethernet::LentSlots<_, 1>>,
    );
    let mut task = core::pin::pin!(core::future::poll_fn(|cx| {
        bridge.poll(cx, |_| {}, |_| panic!("RX refused"), |_| {});
        Poll::<()>::Pending
    }));
    let scheduled = Arc::new(Scheduled(AtomicBool::new(true)));
    let waker = Waker::from(scheduled.clone());
    let mut cx = Context::from_waker(&waker);
    settle(&mut task, &scheduled, &mut cx);
    state.borrow_mut().link(LinkState::Up, 1);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert!(stack.link_is_up());
    state.borrow_mut().receive(9);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert_eq!(stack.receive().unwrap()[14], 9);
    assert_eq!(clock.next_deadline(), None);
    clock.advance(Duration::from_secs(60)).unwrap();
    assert!(!scheduled.0.load(Ordering::Relaxed));
    state.borrow_mut().link(LinkState::Down, 2);
    assert!(scheduled.0.load(Ordering::Relaxed));
    settle(&mut task, &scheduled, &mut cx);
    assert!(!stack.link_is_up());
    // A down/up cycle coalesces to an Up level with a different epoch.
    state.borrow_mut().link(LinkState::Up, 3);
    settle(&mut task, &scheduled, &mut cx);
    assert!(stack.link_is_up());
}
