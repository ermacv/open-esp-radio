//! Path and outcome labels shared by the Core0 task-poll profilers and their
//! zero-sized stand-ins. Always compiled so call sites can name them.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Core0ApRxTurnExit {
    InitialBatch,
    InitialReorder,
    MailboxBlocked,
    TxBlocked,
    BatchPending,
    ReorderPending,
    Drained,
    BudgetExhausted,
}

#[derive(Clone, Copy)]
pub(crate) enum Core0DirectPath {
    Accepted,
    PreflightRejected,
    KeyRejected,
    BankRejected,
    ReorderRejected,
    DuplicateOrIgnored,
}

#[derive(Clone, Copy)]
pub(crate) enum Core0ReorderPath {
    NoKey,
    Inactive,
    Immediate,
    Slow,
}
