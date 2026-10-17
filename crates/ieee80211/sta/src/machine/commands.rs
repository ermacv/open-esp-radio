//! Accepted requests reserve their own terminal cell until it is delivered.

use super::{Bssid, StaCommand, StaDisconnectReason};
use oer_ieee80211_arbiter::AbsenceRefusal;

/// An application request returned intact by refusal or completion. The
/// caller's correlation value is data, not a resource or installation identity.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit this request or retain it for the caller"]
pub struct StaRequest {
    correlation: u64,
    command: StaCommand,
}

impl StaRequest {
    pub const fn new(correlation: u64, command: StaCommand) -> Self {
        Self {
            correlation,
            command,
        }
    }

    pub const fn correlation(&self) -> u64 {
        self.correlation
    }
    pub const fn command(&self) -> StaCommand {
        self.command
    }
}

/// A command which was not admitted; no later terminal is owed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCommandRefused {
    pub request: StaRequest,
    pub reason: CommandRefusal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRefusal {
    /// The same kind of request already owns an operation. Its
    /// `StaAction::CommandCompleted` releases this admission cell.
    Busy,
    /// Its previous terminal must be consumed before another admission.
    /// `Station::poll_action` already has that terminal ready; no new input
    /// or timer is needed before polling it.
    TerminalPending,
    AlreadyConnected,
    NotConnected,
    /// A connection or teardown is still in progress. Wait for the Connect
    /// terminal or the lifecycle `StaNotice::Disconnected` before retrying.
    Transitioning,
}

/// Why a background scan ended before covering its channel list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundScanEnd {
    /// A dwell deadline cannot be represented. The terminal is delivered
    /// after restoring the connected link's receive configuration.
    DeadlineOverflow,
    AbsenceRefused(AbsenceRefusal),
    CancelledAbsences,
    LinkEnded(StaDisconnectReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCommandOutcome {
    Connected {
        bssid: Bssid,
        channel: u8,
    },
    ConnectFailed(StaDisconnectReason),
    Disconnected(StaDisconnectReason),
    BackgroundScan {
        interrupted: Option<BackgroundScanEnd>,
    },
}

/// The unique terminal returning the original accepted request.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "deliver the terminal to the requesting application"]
pub struct StaCommandTerminal {
    pub request: StaRequest,
    pub outcome: StaCommandOutcome,
}

enum CommandCell {
    Vacant,
    Pending(StaRequest),
    Terminal(StaCommandTerminal),
}

/// At most one operation of each command kind can be admitted. Its cell is
/// reserved at admission and cannot be overwritten by a different terminal.
pub(super) struct Commands {
    connect: CommandCell,
    scan: CommandCell,
    disconnect: CommandCell,
}

impl Commands {
    pub(super) const fn new() -> Self {
        Self {
            connect: CommandCell::Vacant,
            scan: CommandCell::Vacant,
            disconnect: CommandCell::Vacant,
        }
    }

    fn cell(&mut self, command: StaCommand) -> &mut CommandCell {
        match command {
            StaCommand::Connect => &mut self.connect,
            StaCommand::BackgroundScan => &mut self.scan,
            StaCommand::Disconnect => &mut self.disconnect,
        }
    }

    pub(super) fn reserve(&mut self, request: StaRequest) -> Result<(), StaCommandRefused> {
        let cell = self.cell(request.command);
        let reason = match cell {
            CommandCell::Vacant => {
                *cell = CommandCell::Pending(request);
                return Ok(());
            }
            CommandCell::Pending(_) => CommandRefusal::Busy,
            CommandCell::Terminal(_) => CommandRefusal::TerminalPending,
        };
        Err(StaCommandRefused { request, reason })
    }

    pub(super) fn complete(&mut self, command: StaCommand, outcome: StaCommandOutcome) {
        let cell = self.cell(command);
        if matches!(cell, CommandCell::Pending(_))
            && let CommandCell::Pending(request) = core::mem::replace(cell, CommandCell::Vacant)
        {
            *cell = CommandCell::Terminal(StaCommandTerminal { request, outcome });
        }
    }

    pub(super) fn poll(&mut self) -> Option<StaCommandTerminal> {
        // A disconnect can cancel a connect and a scan. Return their reserved
        // terminals before the disconnect terminal; none shares a cell.
        for cell in [&mut self.connect, &mut self.scan, &mut self.disconnect] {
            if matches!(cell, CommandCell::Terminal(_))
                && let CommandCell::Terminal(terminal) =
                    core::mem::replace(cell, CommandCell::Vacant)
            {
                return Some(terminal);
            }
        }
        None
    }
}
