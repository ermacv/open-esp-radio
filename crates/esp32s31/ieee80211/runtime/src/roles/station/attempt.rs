//! Concrete ESP32-S31 owner used by the shared station-attempt transaction.
//!
//! This target binding composes the already-qualified channel, join, peer and
//! WPA2 ports. Board code supplies coherent resource groups and a value-only
//! observer type; it does not sequence protocol or hardware phases.

use core::{future::Future, marker::PhantomData};

use crate::{
    datapath::rx::{
        dma::ReceiveDmaStorage,
        frontier::{ReceiveFrontier, RxFrontierError},
    },
    roles::station::{
        join_port::{StaJoinPort, StaJoinRadio, StaJoinRx, StaJoinStation, StaJoinStorage},
        wpa2_port::Wpa2Rx,
    },
};

use oer_esp32s31_hal::owner::RadioRuntimeOwner;

use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyTargetObserver};

use oer_esp32s31_ieee80211::{
    coex::WifiCoexActivity, cooperative_hardware::CooperativeRadioHardware,
};

use oer_esp32s31_ieee80211_mac::{
    crypto::CcmpKeyHardware,
    he::He20PeerHardware,
    init::{MacRuntimeStopHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware},
    rate::control::BeamformingReportHardware,
    rx::RxDma,
    tx::TxHardware,
};

use oer_esp32s31_ieee80211_sta::{
    attempt::{
        StaAttemptConnected, StaAttemptPort, StaAttemptReport, StaAttemptSecurity,
        StaAttemptSecurityExecution, StaAttemptStateError, StaAttemptStation, StaAttemptStepError,
        StaConnectedEntryFailure, StaInstalledSecurity, StaPersonalCredentials,
    },
    join::{StaJoinObserver, StaJoinPortError, StaJoinTransmit},
    peer::{
        ConnectedStaPeer, PreparedStaPeer, ProgrammedStaPeer, StaPeerPort, StaPeerPortError,
        StaPeerRadio, StaPeerStation, StaPeerTransmit,
    },
    profile::select_association,
    wpa2::{
        HandshakeTransmit, InstalledWpa2KeyParts, InstalledWpa2Keys, Wpa2HandshakePort,
        Wpa2HandshakePortError, Wpa2HandshakeRadio, Wpa2HandshakeStorage, Wpa2KeyPort,
        Wpa2KeyPortError, Wpa2KeyRadio, Wpa2KeySession, Wpa2Station,
    },
};

use oer_ieee80211_mac::{
    security::{AssociationAkm, AssociationSecurity, LinkProtection, Pmkid, RsnAssociation},
    station::{AssociationResponse, SelectedRsn, StaSecurityError, select_association_rsn},
};

use oer_ieee80211_sta::{
    join::{StaAuthenticationSuccess, StaJoinError, sae::StaSaeAuthentication},
    station::StaFailureDisposition,
};
use oer_ieee80211_sta_service::join::StaJoinRunner;

use oer_ieee80211_rsn::{
    aes::{RsnSoftwareAes, SoftwareAesKeyUnwrapError},
    runner::{RsnHandshakeConfig, RsnHandshakeError, RsnKeyInstallError},
};
use oer_ieee80211_rsn_service::runner::{
    RsnEstablished, RsnHandshakeRunner, RsnKeyInstallRunner, RsnPendingKeyInstall,
};

mod channel;
mod owner;
mod port;
mod resources;
mod service;

pub use channel::StaAttemptChannel;

pub use owner::{StaAttemptTargetError, StaAttemptTargetOwner};

pub use port::StaAttemptTargetPort;

pub use resources::{StaAttemptRadio, StaAttemptStorage};
