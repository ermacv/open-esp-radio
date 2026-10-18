//! Same-executor transfer to a permanent worker and back to its supervisor.
//! Notifications carry no owners, and completion never implies task-slot reuse.
use core::cell::RefCell;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};

enum State<T, R> {
    Idle,
    Queued(T),
    Running,
    Returned(R),
}

pub(super) struct Exchange<T, R> {
    state: RefCell<State<T, R>>,
    ready: Signal<CriticalSectionRawMutex, ()>,
    completed: Signal<CriticalSectionRawMutex, ()>,
}

impl<T, R> Exchange<T, R> {
    pub const fn new() -> Self {
        Self {
            state: RefCell::new(State::Idle),
            ready: Signal::new(),
            completed: Signal::new(),
        }
    }

    pub fn submit(&self, owner: T) -> Result<(), T> {
        let mut state = self.state.borrow_mut();
        if !matches!(*state, State::Idle) {
            return Err(owner);
        }
        self.completed.reset();
        *state = State::Queued(owner);
        drop(state);
        self.ready.signal(());
        Ok(())
    }

    pub async fn next(&self) -> T {
        self.ready.wait().await;
        let State::Queued(owner) =
            core::mem::replace(&mut *self.state.borrow_mut(), State::Running)
        else {
            panic!("worker wake must have one queued owner");
        };
        owner
    }

    pub fn finish(&self, result: R) {
        let mut state = self.state.borrow_mut();
        assert!(
            matches!(*state, State::Running),
            "only the active worker returns an owner"
        );
        *state = State::Returned(result);
        drop(state);
        self.completed.signal(());
    }

    pub async fn wait_completed(&self) {
        // Returned ownership is durable; the signal is only its notification.
        // A cancelled outer wait may already have consumed that notification.
        while !matches!(*self.state.borrow(), State::Returned(_)) {
            self.completed.wait().await;
        }
    }

    pub fn take_return(&self) -> R {
        assert!(
            matches!(*self.state.borrow(), State::Returned(_)),
            "completion must publish the returned owner first"
        );
        let State::Returned(result) =
            core::mem::replace(&mut *self.state.borrow_mut(), State::Idle)
        else {
            panic!("completion must publish the returned owner first");
        };
        result
    }
}
