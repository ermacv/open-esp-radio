//! Accepted requests reserve their own terminal cell until it is delivered.

use super::{Bssid, StaCommand, StaDisconnectReason};
use oer_ieee80211_core_arbiter::AbsenceRefusal;

use core::{fmt, marker::PhantomData};
use oer_radio_port::{PoolPath, Root};

/// A command kind whose one admission credit is available.
pub enum ConnectCommand {}
pub enum ScanCommand {}
pub enum DisconnectCommand {}

/// One installation/interface's unique permission for command kind `Kind`.
/// Only constructing the station issues these; refusals and terminals return
/// the same permission. The invariant installation lifetime and pool path
/// prevent exchanging credits between stations.
#[must_use = "retain the command credit until submitting its next request"]
pub struct StaCommandCredit<'id, Path: PoolPath, Kind> {
    installation: PhantomData<fn(&'id ()) -> &'id ()>,
    path: PhantomData<fn() -> Path>,
    kind: PhantomData<Kind>,
}

impl<Path: PoolPath, Kind> fmt::Debug for StaCommandCredit<'_, Path, Kind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StaCommandCredit")
    }
}

impl<'id, Path: PoolPath, Kind> StaCommandCredit<'id, Path, Kind> {
    const fn new() -> Self {
        Self {
            installation: PhantomData,
            path: PhantomData,
            kind: PhantomData,
        }
    }
}

/// The three independent credits issued with a newly constructed station.
#[must_use = "the application owns each credit until its request returns"]
pub struct StaCommandCredits<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    pub connect: StaCommandCredit<'id, Path, ConnectCommand>,
    pub scan: StaCommandCredit<'id, Path, ScanCommand>,
    pub disconnect: StaCommandCredit<'id, Path, DisconnectCommand>,
}

/// An admitted command's affine permission, returned with its original request.
#[derive(Debug)]
pub enum StaCommandPermission<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    Connect(StaCommandCredit<'id, Path, ConnectCommand>),
    BackgroundScan(StaCommandCredit<'id, Path, ScanCommand>),
    Disconnect(StaCommandCredit<'id, Path, DisconnectCommand>),
}

impl<Path: PoolPath> StaCommandPermission<'_, Path> {
    pub const fn command(&self) -> StaCommand {
        match self {
            Self::Connect(_) => StaCommand::Connect,
            Self::BackgroundScan(_) => StaCommand::BackgroundScan,
            Self::Disconnect(_) => StaCommand::Disconnect,
        }
    }
}

macro_rules! permission_from {
    ($kind:ty, $variant:ident) => {
        impl<'id, Path: PoolPath> From<StaCommandCredit<'id, Path, $kind>>
            for StaCommandPermission<'id, Path>
        {
            fn from(credit: StaCommandCredit<'id, Path, $kind>) -> Self {
                Self::$variant(credit)
            }
        }
    };
}
permission_from!(ConnectCommand, Connect);
permission_from!(ScanCommand, BackgroundScan);
permission_from!(DisconnectCommand, Disconnect);

/// A request owns its command credit through refusal or terminal delivery.
/// Correlation is application data; the credit's type binds the installation.
#[must_use = "submit this request or retain it for the caller"]
pub struct StaRequest<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    correlation: u64,
    permission: StaCommandPermission<'id, Path>,
}

impl<'id, Path: PoolPath> StaRequest<'id, Path> {
    pub fn new(correlation: u64, permission: impl Into<StaCommandPermission<'id, Path>>) -> Self {
        Self {
            correlation,
            permission: permission.into(),
        }
    }
    pub const fn correlation(&self) -> u64 {
        self.correlation
    }
    pub const fn command(&self) -> StaCommand {
        self.permission.command()
    }
    pub fn into_parts(self) -> (u64, StaCommandPermission<'id, Path>) {
        (self.correlation, self.permission)
    }
}

impl<Path: PoolPath> fmt::Debug for StaRequest<'_, Path> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaRequest")
            .field("correlation", &self.correlation)
            .field("command", &self.command())
            .finish()
    }
}
impl<Path: PoolPath> PartialEq for StaRequest<'_, Path> {
    fn eq(&self, other: &Self) -> bool {
        self.correlation == other.correlation && self.command() == other.command()
    }
}
impl<Path: PoolPath> Eq for StaRequest<'_, Path> {}

/// A command which was not admitted; no later terminal is owed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCommandRefused<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    pub request: StaRequest<'id, Path>,
    pub reason: CommandRefusal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRefusal {
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
pub struct StaCommandTerminal<'id, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    pub request: StaRequest<'id, Path>,
    pub outcome: StaCommandOutcome,
}

enum CommandCell<'id, Path: PoolPath> {
    Vacant,
    Pending(StaRequest<'id, Path>),
    Terminal(StaCommandTerminal<'id, Path>),
}

/// At most one operation of each command kind can be admitted. Its cell is
/// reserved at admission and cannot be overwritten by a different terminal.
pub(super) struct Commands<'id, Path: PoolPath> {
    connect: CommandCell<'id, Path>,
    scan: CommandCell<'id, Path>,
    disconnect: CommandCell<'id, Path>,
}

impl<'id, Path: PoolPath> Commands<'id, Path> {
    pub(super) const fn new() -> (Self, StaCommandCredits<'id, Path>) {
        (
            Self {
                connect: CommandCell::Vacant,
                scan: CommandCell::Vacant,
                disconnect: CommandCell::Vacant,
            },
            StaCommandCredits {
                connect: StaCommandCredit::new(),
                scan: StaCommandCredit::new(),
                disconnect: StaCommandCredit::new(),
            },
        )
    }

    fn cell(&mut self, command: StaCommand) -> &mut CommandCell<'id, Path> {
        match command {
            StaCommand::Connect => &mut self.connect,
            StaCommand::BackgroundScan => &mut self.scan,
            StaCommand::Disconnect => &mut self.disconnect,
        }
    }

    pub(super) fn reserve(&mut self, request: StaRequest<'id, Path>) {
        let cell = self.cell(request.command());
        // Construction consumes the interface's unique resources and issues
        // one credit per kind. Only this cell's refusal/terminal returns it,
        // so a request carrying that credit proves the cell is vacant.
        *cell = CommandCell::Pending(request);
    }

    pub(super) fn complete(&mut self, command: StaCommand, outcome: StaCommandOutcome) {
        let cell = self.cell(command);
        if matches!(cell, CommandCell::Pending(_))
            && let CommandCell::Pending(request) = core::mem::replace(cell, CommandCell::Vacant)
        {
            *cell = CommandCell::Terminal(StaCommandTerminal { request, outcome });
        }
    }

    pub(super) fn poll(&mut self) -> Option<StaCommandTerminal<'id, Path>> {
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
