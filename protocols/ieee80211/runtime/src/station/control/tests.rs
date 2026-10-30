use super::*;
use oer_ieee80211_wire::qos::WmmUserPriority;
use oer_memory::WakingSpscQueue;
use oer_network_port::{EthernetFrameView, FrameLengthError};
use std::{cell::Cell, rc::Rc, vec::Vec};

struct Frame {
    owner: Rc<Cell<usize>>,
    payload: [u8; 4],
}
impl Drop for Frame {
    fn drop(&mut self) {
        self.owner.set(self.owner.get() + 1);
    }
}
impl oer_network_port::EthernetFrame for Frame {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Ok(EthernetFrameView {
            destination: [2; 6],
            source: [4; 6],
            ether_type: 0x0800,
            payload: &self.payload,
        })
    }
}

#[test]
fn neutral_tx_refusal_returns_the_original_owner_and_consumes_no_queue_credit() {
    let mut commands = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
    let mut tx = WakingSpscQueue::<TxMsdu, 1>::new();
    let mut rx = WakingSpscQueue::<EthernetFrame, 1>::new();
    let state = State::new();
    let (sender, _) = commands.split();
    let (_control, _runner) = Control::new(sender, &state).unwrap();
    let (sender, receiver) = tx.split();
    let (_, receive) = rx.split();
    let mut device = Device::new(sender, receive, &state);
    let dropped = Rc::new(Cell::new(0));
    let first = Frame {
        owner: dropped.clone(),
        payload: [7; 4],
    };
    let first = device.try_transmit(first).err().unwrap();
    assert_eq!(first.reason, TransmitError::LinkDown);
    assert_eq!(first.frame.payload, [7; 4]);
    assert!(Rc::ptr_eq(&first.frame.owner, &dropped));
    assert_eq!(dropped.get(), 0);
    let mut status = state.status.get();
    status.link = LinkSnapshot {
        state: LinkState::Up,
        epoch: 31,
    };
    state.status.set(status);
    assert!(device.try_transmit(first.frame).is_ok());
    assert_eq!(dropped.get(), 1);
    let second = Frame {
        owner: dropped.clone(),
        payload: [9; 4],
    };
    let refused = device.try_transmit(second).err().unwrap();
    assert_eq!(refused.reason, TransmitError::QueueFull);
    assert_eq!(refused.frame.payload, [9; 4]);
    assert!(Rc::ptr_eq(&refused.frame.owner, &dropped));
    assert_eq!(dropped.get(), 1);
    assert_eq!(
        receiver.try_receive().unwrap().frame.parts().payload,
        &[7; 4]
    );
    assert!(device.try_transmit(refused.frame).is_ok());
    assert_eq!(
        receiver.try_receive().unwrap().frame.parts().payload,
        &[9; 4]
    );
    assert_eq!(dropped.get(), 2);
}

#[test]
fn cancellation_poisons_control_reuse_and_stop_wait_retains_completion() {
    let mut commands = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
    let state = State::new();
    let (sender, _) = commands.split();
    let (mut control, mut runner) = Control::new(sender, &state).unwrap();
    control.stop();
    embassy_futures::block_on(runner.wait_for_stop());
    let mut cx = Context::from_waker(core::task::Waker::noop());
    assert!(
        core::pin::pin!(control.wait_stopped())
            .poll(&mut cx)
            .is_pending()
    );
    runner.complete();
    embassy_futures::block_on(control.wait_stopped());
    assert!(control.status().stopped);
    let sender = control.try_finish().ok().unwrap();
    let (control, runner) = Control::new(sender, &state).unwrap();
    drop(runner);
    assert!(state.tasks.is_poisoned());
    drop(control);
    assert_eq!(
        state.split().err(),
        Some(ConnectedTaskControlError::Poisoned)
    );
}

#[test]
fn network_link_wakes_on_events_and_keeps_the_cores_epoch() {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::{sync::Arc, task::Wake};
    struct WakeCount(AtomicUsize);
    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut commands = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
    let mut tx = WakingSpscQueue::<TxMsdu, 1>::new();
    let mut rx = WakingSpscQueue::<EthernetFrame, 1>::new();
    let state = State::new();
    let (sender, _) = commands.split();
    let (mut control, _runner) = Control::new(sender, &state).unwrap();
    let (sender, _) = tx.split();
    let (received, receive) = rx.split();
    let mut device = Device::new(sender, receive, &state);
    let wake = Arc::new(WakeCount(AtomicUsize::new(0)));
    let waker = core::task::Waker::from(wake.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(device.poll_link(&mut cx).is_pending());
    let mut delivered = std::vec::Vec::new();
    let mut events = control.events(|event: ServiceOutput<()>| {
        if let ServiceOutput::Notice(notice) = event {
            delivered.push(notice);
        }
    });
    let connected = Notice::Connected {
        bssid: [2; 6],
        channel: 6,
    };
    for epoch in [11, 12] {
        state.epoch(epoch);
        events.emit(ServiceOutput::Notice(connected));
        assert!(wake.0.load(Ordering::Relaxed) != 0);
        assert_eq!(
            device.poll_link(&mut cx),
            Poll::Ready(LinkSnapshot {
                state: LinkState::Up,
                epoch
            })
        );
        assert_eq!(control.status().link, device.link());
        assert!(device.poll_link(&mut cx).is_pending());
        events.emit(ServiceOutput::Notice(Notice::Disconnected(
            super::super::contract::DisconnectReason::LocalRequest,
        )));
        assert_eq!(
            device.poll_link(&mut cx),
            Poll::Ready(LinkSnapshot {
                state: LinkState::Down,
                epoch
            })
        );
    }
    let frame = EthernetFrame::from_parts(oer_ieee80211_wire::data::MsduParts {
        destination: [2; 6],
        source: [4; 6],
        ether_type: 0x0800,
        payload: &[3; 4],
    })
    .unwrap();
    received.try_send(frame).unwrap();
    assert!(device.poll_receive_ready(&mut cx).is_ready());
    assert_eq!(device.try_receive().unwrap().parts().payload, &[3; 4]);
    assert!(device.try_receive().is_none());
    // Cooperative stop is independent of a full command queue.
    assert!(control.try_command(ServiceCommand::Stop).is_ok());
    control.stop();
    assert_eq!(delivered.len(), 4);
}

/// A network frame of `ether_type` around `payload`.
struct Marked {
    ether_type: u16,
    payload: Vec<u8>,
}
impl oer_network_port::EthernetFrame for Marked {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Ok(EthernetFrameView {
            destination: [2; 6],
            source: [4; 6],
            ether_type: self.ether_type,
            payload: &self.payload,
        })
    }
}

#[test]
fn a_network_frame_requests_the_priority_of_its_marking() {
    let mut commands = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
    let mut tx = WakingSpscQueue::<TxMsdu, 1>::new();
    let mut rx = WakingSpscQueue::<EthernetFrame, 1>::new();
    let state = State::new();
    let (sender, _) = commands.split();
    let (_control, _runner) = Control::new(sender, &state).unwrap();
    let (sender, receiver) = tx.split();
    let (_, receive) = rx.split();
    let mut device = Device::new(sender, receive, &state);
    let mut status = state.status.get();
    status.link = LinkSnapshot {
        state: LinkState::Up,
        epoch: 1,
    };
    state.status.set(status);
    // An IPv4 header of DSCP 46 (EF) and total length 20.
    let mut ef = std::vec![0_u8; 20];
    ef[0] = 0x45;
    ef[1] = 46 << 2;
    ef[3] = 20;
    for (ether_type, payload, priority) in [
        (0x0800, ef, WmmUserPriority::UP6),
        // A truncated IPv4 header requests best effort.
        (0x0800, std::vec![0x45, 46 << 2], WmmUserPriority::UP0),
        // VLAN PCP 5.
        (0x8100, std::vec![0xa0, 0, 0x08, 0x00], WmmUserPriority::UP5),
        (0x0806, std::vec![0; 28], WmmUserPriority::UP0),
    ] {
        assert!(
            device
                .try_transmit(Marked {
                    ether_type,
                    payload,
                })
                .is_ok()
        );
        assert_eq!(receiver.try_receive().unwrap().priority, priority);
    }
}
