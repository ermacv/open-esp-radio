#![expect(
    clippy::fn_params_excessive_bools,
    reason = "step 16: the PHY's procedures in the one-loop form"
)]
#![cfg_attr(
    test,
    expect(
        clippy::struct_excessive_bools,
        reason = "step 16: the PHY's procedures in the one-loop form"
    )
)]
//! PHY analog and PBus operations over chip-owned register ports.
#![expect(
    clippy::cast_possible_truncation,
    reason = "step 16: the PHY's procedures in the one-loop form"
)]
#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

#[cfg(test)]
extern crate std;

pub mod analog_i2c;
pub mod i2c;
pub mod serial;
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
