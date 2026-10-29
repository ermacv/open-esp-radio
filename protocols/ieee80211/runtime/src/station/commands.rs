//! Installation-independent requests; affine permissions never leave Runner.
use oer_ieee80211_sta::machine::{
    self, StaCommand, StaCommandCredit, StaCommandCredits, StaCommandPermission, StaRequest,
};
use oer_radio_port::PoolPath;

/// Application correlation and command kind, independent of any installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    correlation: u64,
    command: StaCommand,
}
impl Request {
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
#[derive(Debug, Eq, PartialEq)]
pub struct CommandRefused {
    pub request: Request,
    pub reason: machine::CommandRefusal,
}
#[derive(Debug, Eq, PartialEq)]
pub struct CommandTerminal {
    pub request: Request,
    pub outcome: machine::StaCommandOutcome,
}

pub(super) struct CommandCredits<'id, P: PoolPath> {
    connect: Option<StaCommandCredit<'id, P, machine::ConnectCommand>>,
    scan: Option<StaCommandCredit<'id, P, machine::ScanCommand>>,
    disconnect: Option<StaCommandCredit<'id, P, machine::DisconnectCommand>>,
}
impl<'id, P: PoolPath> CommandCredits<'id, P> {
    #[cfg(feature = "placement")]
    pub const fn empty() -> Self {
        Self {
            connect: None,
            scan: None,
            disconnect: None,
        }
    }
    pub fn new(c: StaCommandCredits<'id, P>) -> Self {
        Self {
            connect: Some(c.connect),
            scan: Some(c.scan),
            disconnect: Some(c.disconnect),
        }
    }
    pub fn request(&mut self, r: Request) -> Option<StaRequest<'id, P>> {
        let permission = match r.command {
            StaCommand::Connect => self.connect.take().map(StaCommandPermission::Connect),
            StaCommand::BackgroundScan => {
                self.scan.take().map(StaCommandPermission::BackgroundScan)
            }
            StaCommand::Disconnect => self.disconnect.take().map(StaCommandPermission::Disconnect),
        }?;
        Some(StaRequest::new(r.correlation, permission))
    }
    fn returned(&mut self, r: StaRequest<'id, P>) -> Request {
        let request = Request::new(r.correlation(), r.command());
        match r.into_parts().1 {
            StaCommandPermission::Connect(c) => self.connect = Some(c),
            StaCommandPermission::BackgroundScan(c) => self.scan = Some(c),
            StaCommandPermission::Disconnect(c) => self.disconnect = Some(c),
        }
        request
    }
    pub fn refused(&mut self, r: machine::StaCommandRefused<'id, P>) -> CommandRefused {
        CommandRefused {
            request: self.returned(r.request),
            reason: r.reason,
        }
    }
    pub fn completed(&mut self, r: machine::StaCommandTerminal<'id, P>) -> CommandTerminal {
        CommandTerminal {
            request: self.returned(r.request),
            outcome: r.outcome,
        }
    }
}
