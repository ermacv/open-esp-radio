//! Numeric retained fault reasons. Text is decoded only by next-boot reporting.
use oer_espressif_interrupt_table::BindFaultCode;

/// Stable software diagnostic code; capture never borrows cached reason text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FaultReason {
    OutsideExecutor = 1,
    SignalContext = 2,
    ExecutorClaimed = 3,
    LinePublished = 4,
    UrgentPublished = 6,
    CounterPublished = 7,
    CounterLatchStuck = 8,
    TimerInstalled = 9,
    TimerBinding = 11,
    TimerBankUnavailable = 12,
    AlarmHart = 13,
    MissingComparator = 14,
    RouteWrongControlBits = 15,
    RouteWrongInterruptCount = 16,
    RouteWrongLevelBits = 17,
    RouteWrongHart = 18,
    RouteNotInTable = 19,
    RouteForeignRoute = 20,
    RouteWrongLine = 21,
    RouteNoSuchSource = 22,
    RouteNoSuchLine = 23,
    RouteNoSuchLevel = 24,
    RouteBindable = 25,
    HilReceiverLimit = 26,
    HilConsole = 27,
    HilCheckpoint = 28,
    HilRateOverflow = 29,
    RouteWrongClass = 31,
    RouteHandlerCellMismatch = 32,
    /// Explicit host-requested HIL reset, not a hardware fault. Observations:
    /// request ID, acknowledgement sequence, session ID low/high words.
    HilRequested = 33,
    /// A software request lost its mandatory response; no hardware claim.
    /// Observations: request kind/site, doorbell slot index, caller epoch (or zero
    /// when no epoch is held), wrapping per-slot post sequence.
    RequestUnanswered = 34,
    BindingRefused = 256,
    BindingStorageClaimed = 257,
    BindingHandlerOccupied = 258,
    BindingLineRefused = 259,
    BindingRouteRefused = 260,
}

impl FaultReason {
    pub const fn class(self) -> oer_diagnostics::fault::Class {
        match self {
            Self::ExecutorClaimed => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::LinePublished => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::UrgentPublished => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::CounterPublished => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::CounterLatchStuck => oer_diagnostics::fault::Class::UnexpectedHardware,
            Self::TimerInstalled => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::TimerBinding => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::TimerBankUnavailable => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::AlarmHart => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::MissingComparator => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::RouteWrongControlBits => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteWrongInterruptCount => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteWrongLevelBits => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteWrongHart => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteNotInTable => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteForeignRoute => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteWrongLine => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteNoSuchSource => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteNoSuchLine => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteNoSuchLevel => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteBindable => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::HilReceiverLimit => oer_diagnostics::fault::Class::Exhaustion,
            Self::HilConsole => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::HilCheckpoint => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::HilRateOverflow => oer_diagnostics::fault::Class::Exhaustion,
            Self::RouteWrongClass => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::RouteHandlerCellMismatch => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::HilRequested => oer_diagnostics::fault::Class::RequestedReset,
            Self::RequestUnanswered => oer_diagnostics::fault::Class::Cancellation,
            Self::BindingRefused => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::BindingStorageClaimed => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::BindingHandlerOccupied => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::BindingLineRefused => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::BindingRouteRefused => oer_diagnostics::fault::Class::ConfigurationBoot,
            Self::OutsideExecutor => oer_diagnostics::fault::Class::SoftwareInvariant,
            Self::SignalContext => oer_diagnostics::fault::Class::SoftwareInvariant,
        }
    }

    pub(super) const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::OutsideExecutor),
            2 => Some(Self::SignalContext),
            3 => Some(Self::ExecutorClaimed),
            4 => Some(Self::LinePublished),
            6 => Some(Self::UrgentPublished),
            7 => Some(Self::CounterPublished),
            8 => Some(Self::CounterLatchStuck),
            9 => Some(Self::TimerInstalled),
            11 => Some(Self::TimerBinding),
            12 => Some(Self::TimerBankUnavailable),
            13 => Some(Self::AlarmHart),
            14 => Some(Self::MissingComparator),
            15 => Some(Self::RouteWrongControlBits),
            16 => Some(Self::RouteWrongInterruptCount),
            17 => Some(Self::RouteWrongLevelBits),
            18 => Some(Self::RouteWrongHart),
            19 => Some(Self::RouteNotInTable),
            20 => Some(Self::RouteForeignRoute),
            21 => Some(Self::RouteWrongLine),
            22 => Some(Self::RouteNoSuchSource),
            23 => Some(Self::RouteNoSuchLine),
            24 => Some(Self::RouteNoSuchLevel),
            25 => Some(Self::RouteBindable),
            26 => Some(Self::HilReceiverLimit),
            27 => Some(Self::HilConsole),
            28 => Some(Self::HilCheckpoint),
            29 => Some(Self::HilRateOverflow),
            31 => Some(Self::RouteWrongClass),
            32 => Some(Self::RouteHandlerCellMismatch),
            33 => Some(Self::HilRequested),
            34 => Some(Self::RequestUnanswered),
            256 => Some(Self::BindingRefused),
            257 => Some(Self::BindingStorageClaimed),
            258 => Some(Self::BindingHandlerOccupied),
            259 => Some(Self::BindingLineRefused),
            260 => Some(Self::BindingRouteRefused),
            _ => None,
        }
    }

    // Only next-boot reporting calls this; fault capture records the enum code.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::OutsideExecutor => "OutsideExecutor",
            Self::SignalContext => "SignalContext",
            Self::ExecutorClaimed => "ExecutorClaimed",
            Self::LinePublished => "LinePublished",
            Self::UrgentPublished => "UrgentPublished",
            Self::CounterPublished => "CounterPublished",
            Self::CounterLatchStuck => "CounterLatchStuck",
            Self::TimerInstalled => "TimerInstalled",
            Self::TimerBinding => "TimerBinding",
            Self::TimerBankUnavailable => "TimerBankUnavailable",
            Self::AlarmHart => "AlarmHart",
            Self::MissingComparator => "MissingComparator",
            Self::RouteWrongControlBits => "RouteWrongControlBits",
            Self::RouteWrongInterruptCount => "RouteWrongInterruptCount",
            Self::RouteWrongLevelBits => "RouteWrongLevelBits",
            Self::RouteWrongHart => "RouteWrongHart",
            Self::RouteNotInTable => "RouteNotInTable",
            Self::RouteForeignRoute => "RouteForeignRoute",
            Self::RouteWrongLine => "RouteWrongLine",
            Self::RouteNoSuchSource => "RouteNoSuchSource",
            Self::RouteNoSuchLine => "RouteNoSuchLine",
            Self::RouteNoSuchLevel => "RouteNoSuchLevel",
            Self::RouteBindable => "RouteBindable",
            Self::HilReceiverLimit => "HilReceiverLimit",
            Self::HilConsole => "HilConsole",
            Self::HilCheckpoint => "HilCheckpoint",
            Self::HilRateOverflow => "HilRateOverflow",
            Self::RouteWrongClass => "RouteWrongClass",
            Self::RouteHandlerCellMismatch => "RouteHandlerCellMismatch",
            Self::HilRequested => "HilRequested",
            Self::RequestUnanswered => "RequestUnanswered",
            Self::BindingRefused => "BindingRefused",
            Self::BindingStorageClaimed => "BindingStorageClaimed",
            Self::BindingHandlerOccupied => "BindingHandlerOccupied",
            Self::BindingLineRefused => "BindingLineRefused",
            Self::BindingRouteRefused => "BindingRouteRefused",
        }
    }
}

impl From<BindFaultCode> for FaultReason {
    fn from(code: BindFaultCode) -> Self {
        match code {
            BindFaultCode::Refused => Self::BindingRefused,
            BindFaultCode::StorageClaimed => Self::BindingStorageClaimed,
            BindFaultCode::HandlerOccupied => Self::BindingHandlerOccupied,
            BindFaultCode::LineRefused => Self::BindingLineRefused,
            BindFaultCode::RouteRefused => Self::BindingRouteRefused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FaultReason;
    #[test]
    fn software_request_abandonment_round_trips_its_retained_code() {
        let reason = FaultReason::RequestUnanswered;
        assert_eq!(FaultReason::from_code(reason as u32), Some(reason));
        assert_eq!(reason.name(), "RequestUnanswered");
    }
}
