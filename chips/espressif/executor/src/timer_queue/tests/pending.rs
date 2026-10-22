use super::*;
use oer_time::{Clock, Duration};
use std::sync::atomic::{AtomicBool, AtomicU32};

struct State {
    clock: oer_time_virtual::VirtualClock<1>,
    pending: &'static PendingBit,
    peer: Mutex<Option<Waker>>,
    observer: Mutex<Option<Waker>>,
    peer_ready: AtomicBool,
    observer_ready: AtomicBool,
    conditional: AtomicBool,
    atoms: AtomicU32,
    atoms_before_peer: AtomicU32,
    observed_pending: AtomicBool,
}

struct Peer(Arc<State>);
impl Future for Peer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let state = &self.0;
        *state.peer.lock().unwrap() = Some(cx.waker().clone());
        if state.peer_ready.swap(false, Ordering::AcqRel) {
            state
                .atoms_before_peer
                .store(state.atoms.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        Poll::Pending
    }
}

struct Observer(Arc<State>);
impl Future for Observer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let state = &self.0;
        *state.observer.lock().unwrap() = Some(cx.waker().clone());
        if state.observer_ready.swap(false, Ordering::AcqRel) {
            let start = state.clock.now();
            while state
                .clock
                .now()
                .saturating_duration_since(start)
                .as_micros()
                < 20
            {
                state.clock.advance(Duration::from_micros(5)).unwrap();
                state.atoms.fetch_add(1, Ordering::Relaxed);
                let pending = state.pending.0.load(Ordering::Acquire);
                state.observed_pending.store(pending, Ordering::Relaxed);
                if !state.conditional.load(Ordering::Relaxed) || pending {
                    break;
                }
            }
        }
        Poll::Pending
    }
}

#[test]
fn cleared_interrupt_bit_misses_a_ready_peer_in_the_current_executor_batch() {
    let pending = Box::leak(Box::new(PendingBit(AtomicBool::new(false))));
    let executor = Box::leak(Box::new(Executor::new(core::ptr::from_mut(pending).cast())));
    let state = Arc::new(State {
        clock: oer_time_virtual::VirtualClock::new(),
        pending,
        peer: Mutex::new(None),
        observer: Mutex::new(None),
        peer_ready: AtomicBool::new(false),
        observer_ready: AtomicBool::new(false),
        conditional: AtomicBool::new(false),
        atoms: AtomicU32::new(0),
        atoms_before_peer: AtomicU32::new(0),
        observed_pending: AtomicBool::new(false),
    });
    let peer = Box::leak(Box::new(TaskStorage::<Peer>::new()));
    executor
        .spawner()
        .spawn(peer.spawn(|| Peer(state.clone())).unwrap());
    let observer = Box::leak(Box::new(TaskStorage::<Observer>::new()));
    executor
        .spawner()
        .spawn(observer.spawn(|| Observer(state.clone())).unwrap());
    poll(executor);

    for (conditional, expected_atoms) in [(false, 1), (true, 4)] {
        state.conditional.store(conditional, Ordering::Relaxed);
        state.atoms.store(0, Ordering::Relaxed);
        state.peer_ready.store(true, Ordering::Release);
        state.observer_ready.store(true, Ordering::Release);
        state.peer.lock().unwrap().as_ref().unwrap().wake_by_ref();
        state
            .observer
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .wake_by_ref();
        assert!(pending.0.load(Ordering::Acquire));
        // The real IRQ handler acknowledges before raw::Executor::poll.
        pending.0.store(false, Ordering::Release);
        poll(executor);
        assert!(!state.observed_pending.load(Ordering::Relaxed));
        assert_eq!(
            state.atoms_before_peer.load(Ordering::Relaxed),
            expected_atoms
        );
        assert!(!state.peer_ready.load(Ordering::Acquire));
    }
}
