//! The image's one shared cleanup after the RX walker started (the shape
//! of the rx-rebind and beacon-stamp probes'): every path (the end of the
//! listening window, a fatal RAW right after the start, an error) requests
//! the MAC channel stop, waits for it up to `MAC_STOP_MS` counted from that
//! request, and then always stops the walker. A failed or overflowing wait
//! is kept as the MAC stop's result and never skips the walker stop. The
//! final checks run only on a confirmed walker stop; the run completes only
//! when the window ran out, the MAC went inactive, the walker stopped and
//! the final checks passed. Every cause is kept together.
//!
//! Without a confirmed walker stop the static ring and its buffers stay
//! held for the rest of the image's life: nothing after the run touches
//! them, whether or not the board is reset afterwards.

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

/// Whether the final checks ran, and how they ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Finish<E> {
    /// The walker stop was not confirmed: nothing was checked.
    NotRun,
    Passed,
    Failed(E),
}

/// The run's end after the cleanup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome<E> {
    /// The window ran out, both stops confirmed, the final checks passed.
    Completed,
    /// Anything else, with every cause kept.
    Failed {
        /// What ended the measurement, if it did not complete.
        cause: Option<E>,
        mac: MacStop,
        dma: DmaStop,
        finish: Finish<E>,
    },
}

/// Combine the measurement's result with both stops; `finish` (the final
/// checks) runs only after a confirmed walker stop.
pub fn conclude<E>(
    measured: Result<(), E>,
    mac: MacStop,
    dma: DmaStop,
    finish: impl FnOnce() -> Result<(), E>,
) -> RunOutcome<E> {
    let finish = match dma {
        DmaStop::Unconfirmed => Finish::NotRun,
        DmaStop::Stopped => match finish() {
            Ok(()) => Finish::Passed,
            Err(error) => Finish::Failed(error),
        },
    };
    match (measured, mac, dma, finish) {
        (Ok(()), MacStop::Inactive, DmaStop::Stopped, Finish::Passed) => RunOutcome::Completed,
        (measured, mac, dma, finish) => RunOutcome::Failed {
            cause: measured.err(),
            mac,
            dma,
            finish,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            self.active_polls = self.active_polls.saturating_sub(1);
            true
        }
        fn deadline(&mut self) -> Option<u32> {
            self.events.push("deadline");
            (!self.deadline_overflows).then_some(self.ticks.saturating_add(MAC_STOP_MS))
        }
        fn passed(&mut self, deadline: &u32) -> bool {
            self.ticks >= *deadline
        }
        async fn tick(&mut self) -> Result<(), ()> {
            if self.tick_overflows {
                return Err(());
            }
            self.ticks = self.ticks.saturating_add(1);
            Ok(())
        }
        fn stop_walker(&mut self) -> bool {
            self.events.push("walker-stop");
            self.walker_stops
        }
    }

    const ORDER: [&str; 3] = ["mac-stop-request", "deadline", "walker-stop"];

    #[test]
    fn a_window_with_both_stops_and_the_checks_completes() {
        let mut port = Port::new();
        port.active_polls = 3;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::Inactive, DmaStop::Stopped));
        assert_eq!(port.events, ORDER);
        assert_eq!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(())),
            RunOutcome::Completed
        );
    }

    #[test]
    fn the_deadline_counts_from_the_stop_request() {
        let mut port = Port::new();
        port.ticks = 1_000;
        port.active_polls = MAC_STOP_MS - 1;
        assert_eq!(ready(shut_down(&mut port)).0, MacStop::Inactive);
    }

    /// Every exit after the walker start: the same cleanup, the cause kept.
    #[test]
    fn every_exit_after_the_start_stops_both_and_keeps_its_cause() {
        for cause in ["initial-fatal", "window-deadline", "console-write"] {
            let mut port = Port::new();
            let (mac, dma) = ready(shut_down(&mut port));
            assert_eq!(port.events, ORDER, "{cause}");
            assert_eq!(
                conclude(Err(cause), mac, dma, || Ok(())),
                RunOutcome::Failed {
                    cause: Some(cause),
                    mac: MacStop::Inactive,
                    dma: DmaStop::Stopped,
                    finish: Finish::Passed,
                }
            );
        }
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
        assert_eq!(port.events, ORDER);
        let mut port = Port::new();
        port.tick_overflows = true;
        port.active_polls = 1;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::WaitOverflow, DmaStop::Stopped));
        assert!(port.events.contains(&"walker-stop"));
        assert_eq!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(())),
            RunOutcome::Failed {
                cause: None,
                mac: MacStop::WaitOverflow,
                dma: DmaStop::Stopped,
                finish: Finish::Passed,
            }
        );
    }

    #[test]
    fn a_mac_still_active_is_not_cancelled_by_a_walker_stop() {
        let mut port = Port::new();
        port.active_polls = u32::MAX;
        let (mac, dma) = ready(shut_down(&mut port));
        assert_eq!((mac, dma), (MacStop::StillActive, DmaStop::Stopped));
        assert_eq!(port.ticks, MAC_STOP_MS);
        assert!(matches!(
            conclude::<&str>(Ok(()), mac, dma, || Ok(())),
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
            Ok(())
        });
        assert!(!checked);
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                cause: None,
                mac: MacStop::Inactive,
                dma: DmaStop::Unconfirmed,
                finish: Finish::NotRun,
            }
        );
    }

    #[test]
    fn an_error_a_mac_timeout_and_an_unconfirmed_walker_keep_every_cause() {
        let mut port = Port::new();
        port.active_polls = u32::MAX;
        port.walker_stops = false;
        let (mac, dma) = ready(shut_down(&mut port));
        let outcome = conclude(Err("window-deadline"), mac, dma, || Ok(()));
        assert_eq!(
            outcome,
            RunOutcome::Failed {
                cause: Some("window-deadline"),
                mac: MacStop::StillActive,
                dma: DmaStop::Unconfirmed,
                finish: Finish::NotRun,
            }
        );
    }

    #[test]
    fn a_final_check_error_is_kept_beside_the_cause_and_the_stops() {
        assert_eq!(
            conclude(
                Err("console-write"),
                MacStop::Inactive,
                DmaStop::Stopped,
                || Err("ring-count")
            ),
            RunOutcome::Failed {
                cause: Some("console-write"),
                mac: MacStop::Inactive,
                dma: DmaStop::Stopped,
                finish: Finish::Failed("ring-count"),
            }
        );
    }
}
