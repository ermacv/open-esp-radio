//! Bluetooth's one membership in the shared RF domain.
//!
//! Only lifecycle commands cross this cell. The backend retains the
//! controller's branded arena and every hardware owner in its P2 context.

use core::convert::Infallible;

use oer_bluetooth_le_radio::{LeBand, TxPower};
use oer_esp32s31_hal::bluetooth::engine::LeTxPower;
use oer_radio_regulatory::{Initiate, Permitted, PowerLimits, TxCeiling};

use crate::{Client, PhyPreparation, sealed};

pub const BIT: u32 = 1 << 1;

/// All Bluetooth roles and connections share this client.
#[derive(Debug)]
pub enum Bluetooth {}

impl sealed::Sealed for Bluetooth {}

impl Client for Bluetooth {
    type Join = BluetoothJoin;
    type Joined = BluetoothJoined;
    type JoinReturned = BluetoothJoin;
    type Track = Installation;
    type Tracked = Installation;
    type TrackReturned = Installation;
    type Retune = Infallible;
    type Retuned = Infallible;
    type RetuneReturned = Infallible;
    type Leave = Installation;
    type Left = BluetoothLeft;

    fn join_returned(join: Self::Join) -> Self::JoinReturned {
        join
    }
    fn track_returned(track: Self::Track) -> Self::TrackReturned {
        track
    }
    fn retune_returned(retune: Infallible) -> Infallible {
        retune
    }
}

/// RF operations checked against the installed band's radiation permission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    PassiveScan,
    DtmRx,
    Advertising,
    ActiveScan,
    CreateConnection,
    Connection,
    DtmTx,
}

/// Installation-wide band and conducted power authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permission {
    band: Permitted<LeBand>,
    ceiling: TxCeiling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    PassiveOnly,
    AboveCeiling,
    BelowProviderMinimum,
}

impl Permission {
    /// Derive the common ceiling from this exact band's permission.
    pub const fn new(band: Permitted<LeBand>, limits: PowerLimits) -> Self {
        Self {
            ceiling: band.tx_ceiling(limits),
            band,
        }
    }

    pub const fn ceiling(self) -> TxCeiling {
        self.ceiling
    }

    /// Listen-only authority never authorizes an operation that transmits.
    pub const fn authorize(self, operation: Operation) -> Result<(), Refusal> {
        match operation {
            Operation::PassiveScan | Operation::DtmRx => Ok(()),
            _ if matches!(self.band.initiate(), Initiate::PassiveOnly) => Err(Refusal::PassiveOnly),
            _ => Ok(()),
        }
    }

    /// Recheck even a request constructed under a different ceiling, then
    /// round down to a provider level. A provider index is never policy.
    pub const fn tx_power(
        self,
        operation: Operation,
        requested: TxPower,
    ) -> Result<LeTxPower, Refusal> {
        if let Err(error) = self.authorize(operation) {
            return Err(error);
        }
        if requested.dbm() > self.ceiling.dbm() {
            return Err(Refusal::AboveCeiling);
        }
        match LeTxPower::from_dbm(requested.dbm()) {
            Some(power) => Ok(power),
            None => Err(Refusal::BelowProviderMinimum),
        }
    }
}

/// The backend's current non-repeating controller installation number.
/// It identifies a completion; it grants no access to controller memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installation(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothJoin {
    pub permission: Permission,
}

pub struct BluetoothJoined {
    pub installation: Installation,
    pub phy: PhyPreparation,
}

pub struct BluetoothLeft {
    /// True only after controller retirement, PHY leave and power release.
    pub stopped: bool,
}

/// Claim Bluetooth's permanent cell for the same actor as Wi-Fi.
pub fn claim() -> Option<(
    crate::ClientInbox<Bluetooth>,
    crate::MembershipCell<Bluetooth>,
)> {
    ends(&BLUETOOTH)
}

static BLUETOOTH: crate::ClientSlot<Bluetooth> = crate::RequestSlot::new(&crate::DOORBELL, BIT);

fn ends(
    slot: &'static crate::ClientSlot<Bluetooth>,
) -> Option<(
    crate::ClientInbox<Bluetooth>,
    crate::MembershipCell<Bluetooth>,
)> {
    let poster = slot.claim()?;
    Some((
        crate::ClientInbox { slot },
        crate::MembershipCell { poster },
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub fn claim_for_host_test(
    doorbell: &'static crate::Doorbell,
) -> (
    crate::ClientInbox<Bluetooth>,
    crate::MembershipCell<Bluetooth>,
) {
    extern crate std;
    ends(std::boxed::Box::leak(std::boxed::Box::new(
        crate::RequestSlot::new(doorbell, BIT),
    )))
    .expect("a fresh Bluetooth cell has its poster")
}
