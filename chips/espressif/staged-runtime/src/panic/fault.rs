//! Numeric retained fault reasons. Text is decoded only by next-boot reporting.
use oer_interrupt_table::BindFaultCode;

/// Stable software diagnostic code; capture never borrows cached reason text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FaultReason {
    OutsideExecutor = 1,
    SignalContext = 2,
    ExecutorClaimed = 3,
    LinePublished = 4,
    InvalidExecutor = 5,
    UrgentPublished = 6,
    CounterPublished = 7,
    CounterLatchStuck = 8,
    TimerInstalled = 9,
    TimerNotInstalled = 10,
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
    BindingRefused = 256,
    BindingStorageClaimed = 257,
    BindingHandlerOccupied = 258,
    BindingLineRefused = 259,
    BindingRouteRefused = 260,
}

impl FaultReason {
    pub(super) const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::OutsideExecutor),
            2 => Some(Self::SignalContext),
            3 => Some(Self::ExecutorClaimed),
            4 => Some(Self::LinePublished),
            5 => Some(Self::InvalidExecutor),
            6 => Some(Self::UrgentPublished),
            7 => Some(Self::CounterPublished),
            8 => Some(Self::CounterLatchStuck),
            9 => Some(Self::TimerInstalled),
            10 => Some(Self::TimerNotInstalled),
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
            Self::InvalidExecutor => "InvalidExecutor",
            Self::UrgentPublished => "UrgentPublished",
            Self::CounterPublished => "CounterPublished",
            Self::CounterLatchStuck => "CounterLatchStuck",
            Self::TimerInstalled => "TimerInstalled",
            Self::TimerNotInstalled => "TimerNotInstalled",
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
