//! What an installation of a data port still holds: the one value a
//! retirement reads. A port retires (it reinstalls or releases its radio)
//! only when the value is empty, and every refusal that rests on it carries
//! it whole, so a refusal names each class still out with its count.

/// How far a port's radio run is, as [`Outstanding`] reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunPhase {
    /// The radio stopped: the one phase a port retires in.
    Stopped,
    /// A start is under way.
    Starting,
    /// The radio runs.
    Running,
    /// A fault ended the run; what is out comes home as a stop's does.
    Recovering,
    /// A stop is under way.
    Stopping,
    /// A start the device began failed and its receive path stops.
    RollingBack,
}

/// The aggregate arenas, as [`Outstanding`] reads them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaState {
    /// Idle and empty, holding no key right, never faulted: they go back.
    Returnable,
    /// An arena holds a chain, an admission or a key right, or faulted.
    Held,
    /// No arenas are installed.
    Absent,
}

/// What an installation still holds, by class, with `QUEUES` transmit
/// queues. Each count is what the port counts; a class is out when its
/// count is not zero (or its phase, fault or arenas say so).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outstanding<F, const QUEUES: usize> {
    /// The radio run's phase.
    pub phase: RunPhase,
    /// The port's recorded contract fault, the value itself.
    pub fault: Option<F>,
    /// Received units the installation holds: its live buffers, plus lends
    /// and losses not yet settled.
    pub received: usize,
    /// Received units lent to the network and not yet dropped. They do not
    /// hold the installation: they stay counted by the lend credit, which
    /// outlives it, and rejoin the next ring when dropped.
    pub lent: usize,
    /// Key removals waiting for the core to clear their keys.
    pub removals: usize,
    /// Station group keys retained after their removal was asked.
    pub retained_groups: usize,
    /// Items a sink refused that the port keeps.
    pub unconsumed: usize,
    /// Transmit buffers of the installation out of the port's bank.
    pub transmit_out: usize,
    /// Attempts queued, by transmit queue.
    pub queued: [usize; QUEUES],
    /// Attempts running on the hardware.
    pub running: usize,
    /// The aggregate arenas.
    pub arenas: ArenaState,
}

/// The classes of [`Outstanding`], one bit each, as [`Outstanding::classes`]
/// names those that are out.
pub mod class {
    /// The radio has not stopped.
    pub const PHASE: u32 = 1 << 0;
    /// The port recorded a fault.
    pub const FAULT: u32 = 1 << 1;
    /// Received units are held.
    pub const RECEIVED: u32 = 1 << 2;
    /// Key removals wait.
    pub const REMOVALS: u32 = 1 << 3;
    /// A group key is retained.
    pub const RETAINED_GROUPS: u32 = 1 << 4;
    /// Refused items are kept.
    pub const UNCONSUMED: u32 = 1 << 5;
    /// Transmit buffers are out of the bank.
    pub const TRANSMIT_OUT: u32 = 1 << 6;
    /// Attempts are queued.
    pub const QUEUED: u32 = 1 << 7;
    /// Attempts run.
    pub const RUNNING: u32 = 1 << 8;
    /// The arenas do not go back.
    pub const ARENAS: u32 = 1 << 9;
}

impl<F, const QUEUES: usize> Outstanding<F, QUEUES> {
    /// The classes that are out, one bit each ([`class`]). Lent units are
    /// no class: they do not hold the installation.
    pub fn classes(&self) -> u32 {
        let mut classes = 0;
        let mut out = |out: bool, bit: u32| {
            if out {
                classes |= bit;
            }
        };
        out(self.phase != RunPhase::Stopped, class::PHASE);
        out(self.fault.is_some(), class::FAULT);
        out(self.received != 0, class::RECEIVED);
        out(self.removals != 0, class::REMOVALS);
        out(self.retained_groups != 0, class::RETAINED_GROUPS);
        out(self.unconsumed != 0, class::UNCONSUMED);
        out(self.transmit_out != 0, class::TRANSMIT_OUT);
        out(self.queued.iter().any(|queued| *queued != 0), class::QUEUED);
        out(self.running != 0, class::RUNNING);
        out(self.arenas != ArenaState::Returnable, class::ARENAS);
        classes
    }

    /// Whether nothing holds the installation: the port may retire.
    pub fn is_empty(&self) -> bool {
        self.classes() == 0
    }

    /// The same value with its fault mapped by `map`.
    pub fn map_fault<G>(self, map: impl FnOnce(F) -> G) -> Outstanding<G, QUEUES> {
        Outstanding {
            phase: self.phase,
            fault: self.fault.map(map),
            received: self.received,
            lent: self.lent,
            removals: self.removals,
            retained_groups: self.retained_groups,
            unconsumed: self.unconsumed,
            transmit_out: self.transmit_out,
            queued: self.queued,
            running: self.running,
            arenas: self.arenas,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Outstanding<u8, 4> {
        Outstanding {
            phase: RunPhase::Stopped,
            fault: None,
            received: 0,
            lent: 0,
            removals: 0,
            retained_groups: 0,
            unconsumed: 0,
            transmit_out: 0,
            queued: [0; 4],
            running: 0,
            arenas: ArenaState::Returnable,
        }
    }

    /// Each class is out on its own, and lent units hold nothing.
    #[test]
    fn each_class_is_out_on_its_own_and_lent_units_hold_nothing() {
        assert!(empty().is_empty());
        let lent = Outstanding { lent: 3, ..empty() };
        assert!(lent.is_empty(), "lent units do not hold the installation");
        let cases: [(Outstanding<u8, 4>, u32); 10] = [
            (
                Outstanding {
                    phase: RunPhase::Recovering,
                    ..empty()
                },
                class::PHASE,
            ),
            (
                Outstanding {
                    fault: Some(7),
                    ..empty()
                },
                class::FAULT,
            ),
            (
                Outstanding {
                    received: 1,
                    ..empty()
                },
                class::RECEIVED,
            ),
            (
                Outstanding {
                    removals: 1,
                    ..empty()
                },
                class::REMOVALS,
            ),
            (
                Outstanding {
                    retained_groups: 1,
                    ..empty()
                },
                class::RETAINED_GROUPS,
            ),
            (
                Outstanding {
                    unconsumed: 2,
                    ..empty()
                },
                class::UNCONSUMED,
            ),
            (
                Outstanding {
                    transmit_out: 4,
                    ..empty()
                },
                class::TRANSMIT_OUT,
            ),
            (
                Outstanding {
                    queued: [0, 0, 1, 0],
                    ..empty()
                },
                class::QUEUED,
            ),
            (
                Outstanding {
                    running: 1,
                    ..empty()
                },
                class::RUNNING,
            ),
            (
                Outstanding {
                    arenas: ArenaState::Absent,
                    ..empty()
                },
                class::ARENAS,
            ),
        ];
        for (outstanding, bit) in cases {
            assert_eq!(outstanding.classes(), bit, "{outstanding:?}");
            assert!(!outstanding.is_empty());
        }
        let mapped = Outstanding {
            fault: Some(7),
            transmit_out: 2,
            ..empty()
        }
        .map_fault(u32::from);
        assert_eq!(mapped.fault, Some(7_u32));
        assert_eq!(mapped.transmit_out, 2);
    }
}
