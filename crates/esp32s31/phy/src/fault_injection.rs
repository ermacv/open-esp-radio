//! Destructive diagnostic checkpoints in the real PHY lifecycle.
//!
//! No timer, reset, executor or transport is owned here. The application must
//! provide independent termination before arming. One attempt per boot prevents
//! a second request from replacing an unfinished physical obligation. Ordinary
//! builds omit this module and its call sites entirely.
use core::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
    task::Poll,
};

/// Fault selected by a diagnostic application, never a production policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Mode {
    BlockedPoll = 1,
    LostCompletion,
    Restoration,
    Cancelled,
}

/// Actual reached frontier; calibration has already changed PBus hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Boundary {
    Calibration,
    Restoration,
}

/// Boot-local progress, independently queryable before destructive release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Phase {
    Idle,
    Armed,
    Reached,
    Released,
    Cancelled,
}

struct Control(AtomicU32);
impl Control {
    const fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    fn arm(&self, mode: Mode) -> bool {
        self.0
            .compare_exchange(
                0,
                ((mode as u32) << 8) | Phase::Armed as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
    fn phase(&self) -> Phase {
        match self.0.load(Ordering::Acquire) & 0xff {
            0 => Phase::Idle,
            1 => Phase::Armed,
            2 => Phase::Reached,
            3 => Phase::Released,
            4 => Phase::Cancelled,
            _ => unreachable!(),
        }
    }
    fn advance(&self, from: Phase, to: Phase) -> bool {
        let old = self.0.load(Ordering::Acquire);
        old & 0xff == from as u32
            && self
                .0
                .compare_exchange(
                    old,
                    (old & !0xff) | to as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
    }
    fn matches(&self, boundary: Boundary) -> bool {
        let mode = self.0.load(Ordering::Acquire) >> 8;
        mode != 0 && (mode == Mode::Restoration as u32) == (boundary == Boundary::Restoration)
    }
    fn is_mode(&self, mode: Mode) -> bool {
        self.0.load(Ordering::Acquire) >> 8 == mode as u32
    }
}
static CONTROL: Control = Control::new();

/// Arm once. Caller must arrange an independently enforced physical deadline.
pub fn arm(mode: Mode) -> bool {
    CONTROL.arm(mode)
}
/// Observe without acknowledging physical completion or renewing any deadline.
pub fn phase() -> Phase {
    CONTROL.phase()
}
/// Release only a reached checkpoint; cannot cancel or reset the attempt.
pub fn release() -> bool {
    CONTROL.advance(Phase::Reached, Phase::Released)
}

/// Park at the selected real boundary. The diagnostic owner polls this future
/// while servicing its control transport, which releases the destructive fault.
pub async fn checkpoint(boundary: Boundary) {
    if !CONTROL.matches(boundary) {
        return;
    }
    CONTROL.advance(Phase::Armed, Phase::Reached);
    core::future::poll_fn(|cx| {
        if phase() == Phase::Released && CONTROL.is_mode(Mode::BlockedPoll) {
            blocked_poll();
        }
        // Release is external to this future. No IRQ is fabricated or consumed.
        cx.waker().wake_by_ref();
        Poll::<()>::Pending
    })
    .await
}

#[inline(never)]
#[allow(
    clippy::infinite_loop,
    clippy::disallowed_methods,
    reason = "explicit destructive diagnostic; independent board watchdog terminates"
)]
fn blocked_poll() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Poll the actual owner-bearing future. Cancellation drops it only after the
/// selected hardware checkpoint and host release; it never returns fake owners
/// or a successful result. The caller's outer deadline lease remains armed.
pub async fn drive<F: Future>(future: F) -> F::Output {
    drive_with(&CONTROL, future).await
}

async fn drive_with<F: Future>(control: &Control, future: F) -> F::Output {
    let result = {
        let mut future = core::pin::pin!(future);
        core::future::poll_fn(|cx| {
            let result = future.as_mut().poll(cx);
            if control.is_mode(Mode::Cancelled) && control.phase() == Phase::Released {
                return Poll::Ready(None);
            }
            result.map(Some)
        })
        .await
    };
    match result {
        Some(result) => result,
        None => {
            control.advance(Phase::Released, Phase::Cancelled);
            core::future::pending().await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_shot_requires_reached_boundary_and_never_rearms() {
        let c = Control::new();
        assert!(!c.advance(Phase::Reached, Phase::Released));
        assert!(c.arm(Mode::Cancelled));
        assert!(!c.arm(Mode::BlockedPoll));
        assert!(!c.advance(Phase::Reached, Phase::Released));
        assert!(c.matches(Boundary::Calibration));
        assert!(!c.matches(Boundary::Restoration));
        assert!(c.advance(Phase::Armed, Phase::Reached));
        assert!(c.advance(Phase::Reached, Phase::Released));
        assert!(!c.advance(Phase::Reached, Phase::Released));
        assert!(c.advance(Phase::Released, Phase::Cancelled));
        assert!(!c.arm(Mode::Restoration));
    }
    #[test]
    fn restoration_cannot_fire_at_calibration_boundary() {
        let c = Control::new();
        assert!(c.arm(Mode::Restoration));
        assert!(!c.matches(Boundary::Calibration));
        assert!(c.matches(Boundary::Restoration));
    }

    #[test]
    fn cancellation_drops_the_real_child_without_returning_completion() {
        use core::{
            cell::Cell,
            pin::pin,
            task::{Context, Waker},
        };
        struct Owner<'a>(&'a Cell<bool>);
        impl Drop for Owner<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Cell::new(false);
        let c = Control::new();
        assert!(c.arm(Mode::Cancelled));
        let owner = Owner(&dropped);
        let child = async {
            let _owner = owner;
            assert!(c.advance(Phase::Armed, Phase::Reached));
            core::future::pending::<()>().await;
        };
        let mut driven = pin!(drive_with(&c, child));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(driven.as_mut().poll(&mut cx).is_pending());
        assert!(!dropped.get());
        assert!(c.advance(Phase::Reached, Phase::Released));
        assert!(driven.as_mut().poll(&mut cx).is_pending());
        assert!(dropped.get());
        assert_eq!(c.phase(), Phase::Cancelled);
        assert!(driven.as_mut().poll(&mut cx).is_pending());
    }

    #[test]
    fn uninjected_future_preserves_its_exact_result() {
        use core::{
            pin::pin,
            task::{Context, Waker},
        };
        let c = Control::new();
        let mut driven = pin!(drive_with(&c, core::future::ready(Err::<(), _>(17))));
        assert_eq!(
            driven
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(17))
        );
        assert_eq!(c.phase(), Phase::Idle);
    }
}
