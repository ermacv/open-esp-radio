//! PHY analog and PBus operations over chip-owned register ports.
#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic
    )
)]

#[cfg(test)]
extern crate std;

pub mod analog_i2c;
pub mod i2c;
pub mod pbus {
    //! PBus mode and completion sequencing. SOURCE: ROM_REV0_PHY_PBUS.

    /// Owned PBus register atoms.
    pub trait PbusAccess {
        fn set_work_mode(&mut self, enabled: bool);
        fn set_debug_mode(&mut self, enabled: bool);
        fn is_busy(&self) -> bool;
        fn clear_transaction(&mut self);
    }

    /// Work mode's semantic observation, supplied by the same PHY context.
    pub trait PbusContext: PbusAccess {
        fn wifi_baseband_enabled(&self) -> bool;
    }

    /// A command is still owned by the hardware host.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum PbusError {
        Busy,
    }

    /// Clear work mode before setting debug mode, preserving both fresh writes.
    pub fn configure_debug_mode(registers: &mut impl PbusAccess) {
        registers.set_work_mode(false);
        registers.set_debug_mode(true);
    }

    /// Publish both mode writes before observing whether the caller must settle.
    pub fn configure_work_mode(registers: &mut impl PbusContext) -> bool {
        registers.set_debug_mode(false);
        registers.set_work_mode(true);
        registers.wifi_baseband_enabled()
    }

    /// Clear the transaction only after one non-busy observation.
    pub fn try_finish_force_test(registers: &mut impl PbusAccess) -> Result<(), PbusError> {
        if registers.is_busy() {
            return Err(PbusError::Busy);
        }
        registers.clear_transaction();
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{cell::RefCell, vec::Vec};

        #[derive(Debug, PartialEq)]
        enum Edge {
            Work(bool),
            Debug(bool),
            Wifi,
            Busy,
            Clear,
        }
        struct Port {
            events: RefCell<Vec<Edge>>,
            busy: bool,
            enabled: bool,
        }
        impl PbusAccess for Port {
            fn set_work_mode(&mut self, enabled: bool) {
                self.events.borrow_mut().push(Edge::Work(enabled));
            }
            fn set_debug_mode(&mut self, enabled: bool) {
                self.events.borrow_mut().push(Edge::Debug(enabled));
            }
            fn is_busy(&self) -> bool {
                self.events.borrow_mut().push(Edge::Busy);
                self.busy
            }
            fn clear_transaction(&mut self) {
                self.events.borrow_mut().push(Edge::Clear);
            }
        }
        impl PbusContext for Port {
            fn wifi_baseband_enabled(&self) -> bool {
                self.events.borrow_mut().push(Edge::Wifi);
                self.enabled
            }
        }
        #[test]
        fn mode_writes_precede_the_same_single_semantic_observation() {
            for enabled in [false, true] {
                let mut port = Port {
                    events: RefCell::new(Vec::new()),
                    busy: false,
                    enabled,
                };
                configure_debug_mode(&mut port);
                assert_eq!(configure_work_mode(&mut port), enabled);
                assert_eq!(
                    *port.events.borrow(),
                    [
                        Edge::Work(false),
                        Edge::Debug(true),
                        Edge::Debug(false),
                        Edge::Work(true),
                        Edge::Wifi
                    ]
                );
            }
        }
        #[test]
        fn a_busy_transaction_is_never_cleared() {
            let mut port = Port {
                events: RefCell::new(Vec::new()),
                busy: true,
                enabled: true,
            };
            assert_eq!(try_finish_force_test(&mut port), Err(PbusError::Busy));
            assert_eq!(*port.events.borrow(), [Edge::Busy]);
            port.busy = false;
            assert_eq!(try_finish_force_test(&mut port), Ok(()));
            assert_eq!(*port.events.borrow(), [Edge::Busy, Edge::Busy, Edge::Clear]);
        }
    }
}

pub mod settle {
    //! Named minimum PHY settles, at most 20 microseconds.

    /// Stable adopted-clock conversion and one wrapping machine-cycle sample.
    pub trait SettleClock {
        fn cycles(&self, micros: u32) -> Option<u32>;
        fn cycle32(&mut self) -> u32;
    }

    /// Maximum blocking settle admitted by the execution contract.
    pub const MAX_MICROS: u32 = 20;

    /// Refuse zero, long or unclocked settles before the first cycle sample.
    pub fn settle_micros(clock: &mut impl SettleClock, micros: u32) -> bool {
        if micros == 0 || micros > MAX_MICROS {
            return false;
        }
        let Some(cycles) = clock.cycles(micros) else {
            return false;
        };
        let start = clock.cycle32();
        while clock.cycle32().wrapping_sub(start) < cycles {
            #[allow(
                clippy::disallowed_methods,
                reason = "a named settle of at most MAX_MICROS (20 µs), the one busy-wait the execution rules admit"
            )]
            core::hint::spin_loop();
        }
        true
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::VecDeque;

        struct Clock {
            samples: VecDeque<u32>,
            clocked: bool,
        }
        impl SettleClock for Clock {
            fn cycles(&self, micros: u32) -> Option<u32> {
                self.clocked.then_some(micros)
            }
            fn cycle32(&mut self) -> u32 {
                self.samples.pop_front().expect("finite settle sample plan")
            }
        }
        #[test]
        fn short_settle_handles_wrap_and_preemption() {
            for samples in [
                std::vec![u32::MAX - 2, u32::MAX, 1, 18],
                std::vec![100, 1000],
            ] {
                let mut clock = Clock {
                    samples: samples.into(),
                    clocked: true,
                };
                assert!(settle_micros(&mut clock, 20));
                assert!(clock.samples.is_empty());
            }
        }
        #[test]
        fn refused_settles_sample_no_counter() {
            let mut clock = Clock {
                samples: VecDeque::new(),
                clocked: true,
            };
            assert!(!settle_micros(&mut clock, 0));
            assert!(!settle_micros(&mut clock, MAX_MICROS + 1));
            clock.clocked = false;
            assert!(!settle_micros(&mut clock, 1));
        }
    }
}
