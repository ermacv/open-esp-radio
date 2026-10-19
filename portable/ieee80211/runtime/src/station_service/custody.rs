//! Bounded ownership after the first rejected terminal. These values have no
//! machine input interface. The first rejection retains its complete owners;
//! later terminals remain sealed. A full sink returns the next terminal to
//! the port, which retains it and stops delivery through poisoned teardown.

use super::*;

/// Storage selected from the backend's terminal capacity. Insertion either
/// retains the owner or returns that same owner; overflow cannot discard it.
pub trait TerminalStorage<T>: Sized {
    const CAPACITY: usize;
    fn empty() -> Self;
    fn try_push(&mut self, terminal: T) -> Result<(), T>;
    fn pop(&mut self) -> Option<T>;
}

impl<T, const N: usize> TerminalStorage<T> for heapless::Deque<T, N> {
    const CAPACITY: usize = N;

    fn empty() -> Self {
        Self::new()
    }

    fn try_push(&mut self, terminal: T) -> Result<(), T> {
        self.push_back(terminal)
    }

    fn pop(&mut self) -> Option<T> {
        self.pop_front()
    }
}

/// A TX return whose context and memory class disagree. It still owns the
/// original ticket, operation context and complete fresh or Sent MPDU.
pub struct UnroutedTx<'id, B: StationPort<'id>> {
    pub context: TxContext<'id, B::Caps, B::Large, <B::Types as PhaseTypes<'id>>::EpochPath>,
    pub ticket: Slot<
        'id,
        <B::Types as PhaseTypes<'id>>::TicketKind,
        <B::Types as PhaseTypes<'id>>::TicketPath,
    >,
    pub buffer: StationTxBuffer<'id, B>,
    pub outcome: PortReturnedTx<'id, B>,
}

/// The first offending return, with every resource still owned.
pub enum RejectedTerminal<'id, B: StationPort<'id>> {
    Sealed(LowerMacTerminal<'id, B::Types>),
    Transmit(UnroutedTx<'id, B>),
}

pub(super) struct ContractFault<'id, B: StationPort<'id>> {
    pub reason: BackendContractViolation,
    pub terminal: RejectedTerminal<'id, B>,
}

impl<'id, B: StationPort<'id>> ContractFault<'id, B> {
    pub(super) fn sealed(
        reason: BackendContractViolation,
        terminal: LowerMacTerminal<'id, B::Types>,
    ) -> Self {
        Self {
            reason,
            terminal: RejectedTerminal::Sealed(terminal),
        }
    }
}

/// A service's terminal bank. `Clear` means no contract violation occurred;
/// `Quarantined` cannot feed any further terminal into protocol machines.
pub enum TerminalCustody<'id, B: StationPort<'id>> {
    Clear,
    Quarantined(QuarantinedTerminals<'id, B>),
}

/// Fixed storage for the first rejected owner and the port's remaining
/// terminal capacity. Every later terminal stays sealed, in delivery order.
pub struct QuarantinedTerminals<'id, B: StationPort<'id>> {
    violation: BackendContractViolation,
    first: RejectedTerminal<'id, B>,
    later: B::TerminalStorage<LowerMacTerminal<'id, B::Types>>,
}

impl<'id, B: StationPort<'id>> QuarantinedTerminals<'id, B> {
    pub fn violation(&self) -> BackendContractViolation {
        self.violation
    }

    /// Return all owners for teardown, preserving the first failure's cause
    /// and subsequent FIFO order. This does not authorize protocol processing.
    pub fn into_parts(
        self,
    ) -> (
        BackendContractViolation,
        RejectedTerminal<'id, B>,
        B::TerminalStorage<LowerMacTerminal<'id, B::Types>>,
    ) {
        (self.violation, self.first, self.later)
    }

    pub(super) fn retain(
        &mut self,
        terminal: LowerMacTerminal<'id, B::Types>,
    ) -> Result<(), LowerMacTerminal<'id, B::Types>> {
        self.later.try_push(terminal)
    }
}

impl<'id, B: StationPort<'id>> From<ContractFault<'id, B>> for TerminalCustody<'id, B> {
    fn from(fault: ContractFault<'id, B>) -> Self {
        Self::Quarantined(QuarantinedTerminals {
            violation: fault.reason,
            first: fault.terminal,
            later: B::TerminalStorage::empty(),
        })
    }
}

pub(super) enum StepFailure<'id, B: StationPort<'id>> {
    Port(ServiceFault<B>),
    Contract(ContractFault<'id, B>),
}

impl<'id, B: StationPort<'id>> From<ServiceFault<B>> for StepFailure<'id, B> {
    fn from(fault: ServiceFault<B>) -> Self {
        Self::Port(fault)
    }
}

impl<'id, B: StationPort<'id>> From<ContractFault<'id, B>> for StepFailure<'id, B> {
    fn from(fault: ContractFault<'id, B>) -> Self {
        Self::Contract(fault)
    }
}

impl<'id, B: StationPort<'id>> StepFailure<'id, B> {
    pub(super) fn into_custody(self) -> (ServiceFault<B>, TerminalCustody<'id, B>) {
        match self {
            Self::Port(fault) => (fault, TerminalCustody::Clear),
            Self::Contract(fault) => (ServiceFault::Contract(fault.reason), fault.into()),
        }
    }
}
