//! The probe's one shared cleanup after the RX walker started: every path
//! (a measurement, a fatal RAW right after the start, an experiment error)
//! requests the MAC channel stop, waits for it up to `MAC_STOP_MS` counted
//! from that request, and then always stops the walker. A failed or
//! overflowing wait is kept as the MAC stop's result and never skips the
//! walker stop. The final checks run only on a confirmed walker stop; the
//! run passes only when the measurement completed, the MAC went inactive,
//! the walker stopped and the final checks decided a pass. Every cause is
//! kept together.
//!
//! Without a confirmed walker stop the static ring, storage and spares stay
//! held for the rest of the image's life: nothing after the run touches
//! them, whether or not the board is reset afterwards.

use crate::verdict::Verdict;

/// How long the MAC channel may take to report inactive after its stop
/// request.
pub const MAC_STOP_MS: u32 = 50;

/// The MAC channel stop's result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacStop {
    Inactive,
    /// Still active `MAC_STOP_MS` after the request.
    StillActive,
    /// The deadline `MAC_STOP_MS` after the request does not fit the clock.
    DeadlineOverflow,
    /// The next poll's wake time does not fit the clock.
    WaitOverflow,
}

impl MacStop {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::StillActive => "still-active",
            Self::DeadlineOverflow => "deadline-overflow",
            Self::WaitOverflow => "wait-overflow",
        }
    }
}

/// The walker stop's result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaStop {
    Stopped,
    /// The walker did not read back as stopped.
    Unconfirmed,
}

impl DmaStop {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// What the cleanup drives: the MAC channel, a clock for the bounded wait
/// and the walker.
pub trait StopPort {
    type Deadline;
    fn request_mac_stop(&mut self);
    fn mac_active(&mut self) -> bool;
    /// `MAC_STOP_MS` from now; `None` when it does not fit the clock.
    fn deadline(&mut self) -> Option<Self::Deadline>;
    fn passed(&mut self, deadline: &Self::Deadline) -> bool;
    /// Wait one poll period; `Err` when its wake time does not fit.
    async fn tick(&mut self) -> Result<(), ()>;
    /// Stop the walker; whether it read back as stopped.
    fn stop_walker(&mut self) -> bool;
}

/// Stop the MAC channel, then the walker, whatever the MAC stop's result.
pub async fn shut_down<P: StopPort>(port: &mut P) -> (MacStop, DmaStop) {
    port.request_mac_stop();
    // Counted from the request.
    let mac = match port.deadline() {
        None => MacStop::DeadlineOverflow,
        Some(deadline) => loop {
            if !port.mac_active() {
                break MacStop::Inactive;
            }
            if port.passed(&deadline) {
                break MacStop::StillActive;
            }
            if port.tick().await.is_err() {
                break MacStop::WaitOverflow;
            }
        },
    };
    let dma = if port.stop_walker() {
        DmaStop::Stopped
    } else {
        DmaStop::Unconfirmed
    };
    (mac, dma)
}

/// The run's end after the cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome<E> {
    /// Measured, both stops confirmed: the final checks' verdict.
    Decided(Verdict),
    /// Anything else, with every cause kept.
    Failed {
        /// What ended the measurement, if it did not complete.
        cause: Option<E>,
        mac: MacStop,
        dma: DmaStop,
        /// The final checks' error; `None` also when they did not run.
        finish: Option<E>,
        /// The final checks' verdict, when they ran and decided one.
        verdict: Option<Verdict>,
    },
}

/// Combine the measurement's result with both stops; `finish` (the final
/// checks and the verdict) runs only after a confirmed walker stop.
pub fn conclude<E>(
    measured: Result<(), E>,
    mac: MacStop,
    dma: DmaStop,
    finish: impl FnOnce() -> Result<Verdict, E>,
) -> RunOutcome<E> {
    let (finish, verdict) = match dma {
        DmaStop::Unconfirmed => (None, None),
        DmaStop::Stopped => match finish() {
            Ok(verdict) => (None, Some(verdict)),
            Err(error) => (Some(error), None),
        },
    };
    match (measured, mac, dma, finish, verdict) {
        (Ok(()), MacStop::Inactive, DmaStop::Stopped, None, Some(verdict)) => {
            RunOutcome::Decided(verdict)
        }
        (measured, mac, dma, finish, verdict) => RunOutcome::Failed {
            cause: measured.err(),
            mac,
            dma,
            finish,
            verdict,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::Shortfall;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    fn ready<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the test port never waits"),
        }
    }

    /// A MAC that stays active for `active_polls` polls, a clock of
    /// `ticks` that overflows as told, and a walker that stops or not.
    struct Port {
        active_polls: u32,
        deadline_overflows: bool,
        tick_overflows: bool,
        ticks: u32,
        walker_stops: bool,
        events: Vec<&'static str>,
    }

    impl Port {
        fn new() -> Self {
            Self {
                active_polls: 0,
                deadline_overflows: false,
                tick_overflows: false,
                ticks: 0,
                walker_stops: true,
                events: Vec::new(),
            }
        }
    }

    impl StopPort for Port {
        type Deadline = u32;
        fn request_mac_stop(&mut self) {
            self.events.push("mac-stop-request");
        }
        fn mac_active(&mut self) -> bool {
            if self.active_polls == 0 {
                return false;
            }
            self.active_polls -= 1;
            true
        }
        fn deadline(&mut self) -> Option<u32> {
            self.events.push("deadline");
            (!self.deadline_overflows).then_some(self.ticks + MAC_STOP_MS)
        }
        fn passed(&mut self, deadline: &u32) -> bool {
            self.ticks >= *deadline
        }
        async fn tick(&mut self) -> Result<(), ()> {
            if self.tick_overflows {
                return Err(());
            }
            self.ticks += 1;
            Ok(())
        }
        fn stop_walker(&mut self) -> bool {
            self.events.push("walker-stop");
            self.walker_stops
        }
    }

    #[test]
    fn a_completed_measurement_with_both_stops_takes_the_final_verdict() {
        let mut port = Port::new();
        port.active_polls = 3;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::Inactive, DmaStop::Stopped));
        assert_eq!(port.events, ["mac-stop-request", "deadline", "walker-stop"]);
        assert_eq!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(Verdict::Pass)),
            RunOutcome::Decided(Verdict::Pass)
        );
    }

    #[test]
    fn the_deadline_counts_from_the_stop_request() {
        let mut port = Port::new();
        port.ticks = 1_000;
        port.active_polls = MAC_STOP_MS - 1;
        assert_eq!(ready(shut_down(&mut port)).0, MacStop::Inactive);
    }

    #[test]
    fn an_initial_fatal_still_stops_both_and_skips_no_cause() {
        let mut port = Port::new();
        let (mac, dma) = ready(shut_down(&mut port));
        let outcome = conclude(Err("initial-fatal"), mac, dma, || {
            Ok(Verdict::Insufficient(Shortfall::FixedFrames))
        });
        assert_eq!(port.events, ["mac-stop-request", "deadline", "walker-stop"]);
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                cause: Some("initial-fatal"),
                mac: MacStop::Inactive,
                dma: DmaStop::Stopped,
                finish: None,
                verdict: Some(Verdict::Insufficient(Shortfall::FixedFrames)),
            }
        );
    }

    #[test]
    fn deadline_and_wait_overflows_are_kept_and_the_walker_still_stops() {
        let mut port = Port::new();
        port.deadline_overflows = true;
        port.active_polls = 1;
        assert_eq!(
            ready(shut_down(&mut port)),
            (MacStop::DeadlineOverflow, DmaStop::Stopped)
        );
        assert_eq!(port.events, ["mac-stop-request", "deadline", "walker-stop"]);
        let mut port = Port::new();
        port.tick_overflows = true;
        port.active_polls = 1;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::WaitOverflow, DmaStop::Stopped));
        assert!(port.events.contains(&"walker-stop"));
        assert!(matches!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(Verdict::Pass)),
            RunOutcome::Failed {
                mac: MacStop::WaitOverflow,
                verdict: Some(Verdict::Pass),
                ..
            }
        ));
    }

    #[test]
    fn a_mac_still_active_is_not_cancelled_by_a_walker_stop() {
        let mut port = Port::new();
        port.active_polls = u32::MAX;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::StillActive, DmaStop::Stopped));
        assert_eq!(port.ticks, MAC_STOP_MS);
        assert!(matches!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(Verdict::Pass)),
            RunOutcome::Failed {
                cause: None,
                mac: MacStop::StillActive,
                dma: DmaStop::Stopped,
                ..
            }
        ));
    }

    #[test]
    fn an_unconfirmed_walker_stop_fails_without_the_final_checks() {
        let mut port = Port::new();
        port.walker_stops = false;
        let (mac, dma) = ready(shut_down(&mut port));
        let mut checked = false;
        let outcome = conclude::<&str>(Ok(()), mac, dma, || {
            checked = true;
            Ok(Verdict::Pass)
        });
        assert!(!checked);
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                cause: None,
                mac: MacStop::Inactive,
                dma: DmaStop::Unconfirmed,
                finish: None,
                verdict: None,
            }
        );
    }

    #[test]
    fn a_mac_timeout_and_an_unconfirmed_walker_stop_keep_both_and_the_cause() {
        let mut port = Port::new();
        port.active_polls = u32::MAX;
        port.walker_stops = false;
        let (mac, dma) = ready(shut_down(&mut port));
        let outcome = conclude(Err("fatal-raw"), mac, dma, || Ok(Verdict::Pass));
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                cause: Some("fatal-raw"),
                mac: MacStop::StillActive,
                dma: DmaStop::Unconfirmed,
                finish: None,
                verdict: None,
            }
        );
    }

    #[test]
    fn a_final_check_error_is_kept_beside_the_stops() {
        assert_eq!(
            conclude(Ok(()), MacStop::Inactive, DmaStop::Stopped, || Err(
                "retired-read"
            )),
            RunOutcome::Failed {
                cause: None,
                mac: MacStop::Inactive,
                dma: DmaStop::Stopped,
                finish: Some("retired-read"),
                verdict: None,
            }
        );
    }
}
