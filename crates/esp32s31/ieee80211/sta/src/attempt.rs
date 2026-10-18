//! The ESP32-S31 parts of one station attempt.
//!
//! The attempt values, its port and its security material are
//! `oer_ieee80211_sta::attempt` and the transaction that orders the phases is
//! the `StaAttempt` of `oer-ieee80211-sta-service`. This module keeps what
//! binds the ESP32-S31: the channel an attempt programs from the chip
//! association profile, the hardware key slots a completed WPA2 attempt owns
//! and the reports of the chip driver phases.

pub use oer_ieee80211_sta::attempt::{
    AssociationAttemptFailure, AssociationAttemptOutcome, StaAttemptConnected, StaAttemptObserver,
    StaAttemptPort, StaAttemptProgress, StaAttemptSecurity, StaAttemptSecurityExecution,
    StaAttemptSecurityMaterial, StaAttemptStage, StaAttemptStateError, StaAttemptStepError,
    StaConnectedEntryFailure, StaPersonalCredentials,
};
pub use oer_ieee80211_sta::pmksa::StaSharedPmksa;

use crate::connected::management_protection::StationManagementProtection;

use crate::{
    connected_rx::StaCcmpRxReplayEpoch, peer::StaPeerProgrammingReport,
    wpa2::Wpa2HandshakeTelemetry,
};

use oer_esp32s31_ieee80211_mac::{
    crypto::{StaGroupCcmpKeyMaterial, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
    tx::TxCompletion,
};

use {
    oer_ieee80211_mac::channel::WifiChannel,
    oer_ieee80211_mac::channel::WifiChannelError,
    oer_ieee80211_mac::channel::WifiChannelWidth,
    oer_ieee80211_mac::scan::ScanRecord,
    oer_ieee80211_mac::security::{LinkProtection, StaSecurityPolicy},
    oer_ieee80211_mac::station::association::Preference,
};

use oer_ieee80211_rsn::runner::RsnKeyInstallMetadata;
use oer_ieee80211_sta::join::{StaAssociationSuccess, StaAuthenticationSuccess};

/// Immutable local/candidate policy for one attempt.
#[derive(Clone, Copy)]
pub struct StaAttemptStation {
    pub station_address: [u8; 6],
    pub access_point: ScanRecord,
    pub association_preference: Preference,
    pub security: StaSecurityPolicy,
}

impl StaAttemptStation {
    /// Exact portable channel selected by the same policy that programs the
    /// ESP32-S31 PHY. Reclaim and paired-role composition must preserve this
    /// width instead of manufacturing a 20-MHz role context from the primary
    /// channel number alone.
    pub fn selected_channel(&self) -> Result<WifiChannel, WifiChannelError> {
        let selection =
            crate::profile::select_association(&self.access_point, self.association_preference);
        let width = match selection.cbw {
            2 => WifiChannelWidth::Mhz40Above,
            3 => WifiChannelWidth::Mhz40Below,
            _ => WifiChannelWidth::Mhz20,
        };
        WifiChannel::new_2_4_ghz(selection.primary_channel, width)
    }
}

/// Station identity and association policy before candidate selection.
///
/// Keeping this distinct from [`StaAttemptStation`] makes it
/// impossible to enter Authentication/Association with a fabricated empty
/// scan record merely to satisfy an owner layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaIdentity {
    pub station_address: [u8; 6],
    pub association_preference: Preference,
    pub security: StaSecurityPolicy,
}

impl StaIdentity {
    pub const fn select(self, access_point: ScanRecord) -> StaAttemptStation {
        StaAttemptStation {
            station_address: self.station_address,
            access_point,
            association_preference: self.association_preference,
            security: self.security,
        }
    }
}

/// Hardware key ownership created only by a completed WPA2 attempt.
// Keep the installed slots and replay epoch inline: this no-alloc owner must
// return every hardware capability by value when the connected role ends.
#[allow(clippy::large_enum_variant)]
pub enum StaInstalledSecurity {
    Open,
    Wpa2Personal {
        pairwise: StaPairwiseCcmpSlot,
        group: StaGroupCcmpSlot,
        group_material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayEpoch,
        /// Present when the association protects its management frames.
        management: Option<StationManagementProtection>,
    },
}

impl StaInstalledSecurity {
    pub const fn mode(&self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal { .. } => LinkProtection::Ccmp,
        }
    }
}

/// Value-only reports produced by the real driver phases.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaAttemptReport {
    /// Exact security execution selected before association. Open makes the
    /// legacy WPA2-named transaction stages explicit no-ops; it never means
    /// that a handshake or key installation succeeded.
    pub security: Option<StaAttemptSecurityExecution>,
    pub authentication: Option<StaAuthenticationSuccess>,
    /// The SAE association resumed a cached PMKSA instead of running SAE.
    pub pmksa_resumed: bool,
    pub association: Option<StaAssociationSuccess>,
    pub peer: Option<StaPeerProgrammingReport>,
    /// Message-2 progress is retained even when the handshake later fails.
    pub wpa2_handshake: Option<Wpa2HandshakeTelemetry>,
    pub wpa2: Option<RsnKeyInstallMetadata>,
    /// A failed Message 4 status is still useful attempt evidence.
    pub message4: Option<TxCompletion>,
}

#[cfg(test)]
mod tests;
