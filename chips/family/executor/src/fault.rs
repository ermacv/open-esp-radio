//! Executor failures retain their observations and quarantine every owner.

use core::{marker::PhantomData, mem::ManuallyDrop};
use oer_espressif_interrupt_table::Error;
use oer_espressif_staged_runtime::panic::FaultReason;
use oer_riscv::clic::LineState;

/// Source observations of an executor or time-driver invariant failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    OutsideExecutor {
        running: u8,
        level: u8,
    },
    SignalContext {
        executor: u8,
        running: u8,
    },
    ExecutorClaimed {
        executor: u8,
        stamp: usize,
    },
    LinePublished {
        executor: u8,
    },
    UrgentPublished {
        executor: u8,
    },
    CounterPublished,
    CounterLatchStuck,
    TimerInstalled {
        writer: u8,
    },
    TimerBinding {
        executor: u8,
        comparator: u8,
        previous: u8,
    },
    TimerBankUnavailable {
        comparator: u8,
    },
    MissingComparator {
        executor: u8,
        comparator: u8,
    },
    Binding(oer_espressif_interrupt_table::BindFault),
    Route {
        executor: u8,
        error: Error,
    },
    /// A task's waker was woken on another hart than its executor's.
    WakerCrossedHarts {
        executor: u8,
        owner: usize,
        current: usize,
    },
}

impl Fault {
    /// The fault of a wake of a task of `executor`, whose executor runs on
    /// hart `owner` (none before it started), on hart `current`: a waker
    /// stays on its executor's hart.
    pub(crate) fn crossed_wake(executor: u8, owner: Option<usize>, current: usize) -> Option<Self> {
        owner
            .filter(|&owner| owner != current)
            .map(|owner| Self::WakerCrossedHarts {
                executor,
                owner,
                current,
            })
    }

    #[cfg(feature = "chip")]
    pub(crate) fn binding<E: oer_espressif_interrupt_table::Entry>(
        executor: u8,
        error: &oer_espressif_interrupt_table::BindError<E>,
    ) -> Self {
        match error {
            oer_espressif_interrupt_table::BindError::Refused { error, .. } => Self::Route {
                executor,
                error: *error,
            },
            oer_espressif_interrupt_table::BindError::Fault(held) => Self::Binding(held.fault),
        }
    }

    /// A bounded diagnostic, without formatting or loss of source words.
    pub fn diagnostic(self) -> (FaultReason, [u32; 4]) {
        match self {
            Self::OutsideExecutor { running, level } => (
                FaultReason::OutsideExecutor,
                [running.into(), level.into(), 0, 0],
            ),
            Self::SignalContext { executor, running } => (
                FaultReason::SignalContext,
                [executor.into(), running.into(), 0, 0],
            ),
            Self::ExecutorClaimed { executor, stamp } => (
                FaultReason::ExecutorClaimed,
                [executor.into(), stamp as u32, 0, 0],
            ),
            Self::LinePublished { executor } => {
                (FaultReason::LinePublished, [executor.into(), 0, 0, 0])
            }
            Self::UrgentPublished { executor } => {
                (FaultReason::UrgentPublished, [executor.into(), 0, 0, 0])
            }
            Self::CounterPublished => (FaultReason::CounterPublished, [0; 4]),
            Self::CounterLatchStuck => (FaultReason::CounterLatchStuck, [0; 4]),
            Self::TimerInstalled { writer } => {
                (FaultReason::TimerInstalled, [writer.into(), 0, 0, 0])
            }
            Self::TimerBinding {
                executor,
                comparator,
                previous,
            } => (
                FaultReason::TimerBinding,
                [executor.into(), comparator.into(), previous.into(), 0],
            ),
            Self::TimerBankUnavailable { comparator } => (
                FaultReason::TimerBankUnavailable,
                [comparator.into(), 0, 0, 0],
            ),
            Self::WakerCrossedHarts {
                executor,
                owner,
                current,
            } => (
                FaultReason::WakerCrossedHarts,
                [executor.into(), owner as u32, current as u32, 0],
            ),
            Self::MissingComparator {
                executor,
                comparator,
            } => (
                FaultReason::MissingComparator,
                [executor.into(), comparator.into(), 0, 0],
            ),
            Self::Route { executor, error } => route_diagnostic(executor, error),
            Self::Binding(fault) => {
                let (code, words) = fault.diagnostic();
                (code.into(), words)
            }
        }
    }

    /// Mask execution, keep all interrupted/static owners quarantined, retain
    /// this reason and reset through platform `P`'s fatal path. The next
    /// staged boot reports the same words.
    #[cfg(feature = "chip")]
    pub fn reset<P: oer_espressif_platform_port::Platform>(self) -> ! {
        #[cfg(target_arch = "riscv32")]
        {
            let (reason, observations) = self.diagnostic();
            oer_espressif_staged_runtime::panic::reset_fault::<P>(reason, observations)
        }
        // Host type checks have no reset hardware or retained boot record.
        #[cfg(not(target_arch = "riscv32"))]
        P::fatal(|_| ())
    }
}

/// A refused setup keeps its inputs permanently quarantined, even if the
/// caller drops the failure. No retry or owner-recovery authority is issued.
/// `P` is the platform whose fatal path [`Failure::reset`] takes.
pub struct Failure<P, T> {
    pub fault: Fault,
    owners: ManuallyDrop<T>,
    platform: PhantomData<fn() -> P>,
}

impl<P, T> Failure<P, T> {
    pub(crate) fn new(fault: Fault, owners: T) -> Self {
        Self {
            fault,
            owners: ManuallyDrop::new(owners),
            platform: PhantomData,
        }
    }

    pub(crate) fn map_owners<U>(self, map: impl FnOnce(T) -> U) -> Failure<P, U> {
        Failure::new(self.fault, map(ManuallyDrop::into_inner(self.owners)))
    }
}

#[cfg(feature = "chip")]
impl<P: oer_espressif_platform_port::Platform, T> Failure<P, T> {
    /// The image terminates setup while retaining every refused input.
    pub fn reset(self) -> ! {
        self.fault.reset::<P>()
    }
}

impl<P, T> core::fmt::Debug for Failure<P, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Failure")
            .field("fault", &self.fault)
            .finish_non_exhaustive()
    }
}

fn line(state: LineState) -> u32 {
    u32::from(state.enabled)
        | (u32::from(state.vectored) << 1)
        | ((state.trigger as u32) << 2)
        | (u32::from(state.control.bits()) << 8)
}

fn route_diagnostic(executor: u8, error: Error) -> (FaultReason, [u32; 4]) {
    let executor = executor.into();
    match error {
        Error::WrongControlBits { actual, expected } => (
            FaultReason::RouteWrongControlBits,
            [executor, actual.into(), expected.into(), 0],
        ),
        Error::WrongInterruptCount { actual, expected } => (
            FaultReason::RouteWrongInterruptCount,
            [executor, actual.into(), expected.into(), 0],
        ),
        Error::WrongLevelBits { actual, expected } => (
            FaultReason::RouteWrongLevelBits,
            [executor, actual.into(), expected.into(), 0],
        ),
        Error::WrongHart { hart, current } => (
            FaultReason::RouteWrongHart,
            [executor, hart.into(), current.into(), 0],
        ),
        Error::WrongClass {
            source,
            table,
            entry,
        } => (
            FaultReason::RouteWrongClass,
            [executor, source.into(), table as u32, entry as u32],
        ),
        Error::HandlerCellMismatch { source } => (
            FaultReason::RouteHandlerCellMismatch,
            [executor, source.into(), 0, 0],
        ),
        Error::NotInTable { source } => (
            FaultReason::RouteNotInTable,
            [executor, source.into(), 0, 0],
        ),
        Error::ForeignRoute { source, line } => (
            FaultReason::RouteForeignRoute,
            [executor, source.into(), line.into(), 0],
        ),
        Error::WrongLine {
            line: index,
            state,
            expected,
        } => (
            FaultReason::RouteWrongLine,
            [executor, index.into(), line(state), line(expected)],
        ),
        Error::NoSuchSource { source } => (
            FaultReason::RouteNoSuchSource,
            [executor, source.into(), 0, 0],
        ),
        Error::NoSuchLine { line } => (FaultReason::RouteNoSuchLine, [executor, line.into(), 0, 0]),
        Error::NoSuchLevel { level } => (
            FaultReason::RouteNoSuchLevel,
            [executor, level.into(), 0, 0],
        ),
        Error::Bindable { source } => (FaultReason::RouteBindable, [executor, source.into(), 0, 0]),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::Cell;

    #[test]
    fn refused_and_remapped_inputs_are_never_released() {
        struct Owner<'a>(&'a Cell<u32>);
        impl Drop for Owner<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Cell::new(0);
        let failure = Failure::<(), _>::new(Fault::CounterLatchStuck, Owner(&drops));
        let failure = failure.map_owners(|owner| (owner, 7));
        assert_eq!(failure.fault, Fault::CounterLatchStuck);
        drop(failure);
        assert_eq!(
            drops.get(),
            0,
            "dropping a refusal cannot release its owner"
        );
    }

    #[test]
    fn a_wake_on_another_hart_than_its_executors_is_a_fault() {
        assert_eq!(Fault::crossed_wake(1, Some(1), 1), None);
        assert_eq!(Fault::crossed_wake(1, None, 0), None, "not started");
        let fault = Fault::crossed_wake(1, Some(1), 0).unwrap();
        assert_eq!(
            fault.diagnostic(),
            (FaultReason::WakerCrossedHarts, [1, 1, 0, 0])
        );
    }

    #[test]
    fn route_fault_keeps_every_source_observation() {
        let fault = Fault::Route {
            executor: 3,
            error: Error::WrongHart {
                hart: 0xfe,
                current: 0xfd,
            },
        };
        assert_eq!(
            fault.diagnostic(),
            (FaultReason::RouteWrongHart, [3, 254, 253, 0])
        );
        let fault = Fault::ExecutorClaimed {
            executor: 2,
            stamp: 0xfedc_ba98,
        };
        assert_eq!(
            fault.diagnostic(),
            (FaultReason::ExecutorClaimed, [2, 0xfedc_ba98, 0, 0])
        );
    }

    #[test]
    fn binding_identity_refusals_keep_their_typed_reason_and_classes() {
        assert_eq!(
            route_diagnostic(
                3,
                Error::WrongClass {
                    source: 5,
                    table: oer_espressif_interrupt_table::Class::Hardware,
                    entry: oer_espressif_interrupt_table::Class::Executor,
                }
            ),
            (FaultReason::RouteWrongClass, [3, 5, 0, 1])
        );
        assert_eq!(
            route_diagnostic(3, Error::HandlerCellMismatch { source: 5 }),
            (FaultReason::RouteHandlerCellMismatch, [3, 5, 0, 0])
        );
    }
}
