//! Receive metadata of one MPDU.

use crate::Ieee80211Stamp;
use oer_protocols_ieee80211_wire::{channel::Channel, phy::PhyRate};

/// Provenance of one receive value.
///
/// Missing data is explicit because deriving a value from an active Block
/// Ack agreement, a protected frame-control bit, or another adjacent
/// condition is not equivalent to observing it in hardware or validating it
/// in the protocol parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxEvidence<T> {
    /// The receive backend decoded this value from a documented hardware
    /// status field.
    HardwareObserved(T),
    /// Portable protocol processing established this value while validating
    /// the frame.
    ProtocolValidated(T),
    /// Neither layer has evidence for this value at the current boundary.
    Unavailable,
}

impl<T> RxEvidence<T> {
    /// Borrow the value without changing its provenance.
    pub const fn as_ref(&self) -> RxEvidence<&T> {
        match self {
            Self::HardwareObserved(value) => RxEvidence::HardwareObserved(value),
            Self::ProtocolValidated(value) => RxEvidence::ProtocolValidated(value),
            Self::Unavailable => RxEvidence::Unavailable,
        }
    }

    /// Whether either hardware or protocol evidence carries a value.
    pub const fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }

    /// The value with its provenance kept; `Unavailable` when `convert`
    /// finds no portable value in it.
    pub fn and_then<U>(self, convert: impl FnOnce(T) -> Option<U>) -> RxEvidence<U> {
        match self {
            Self::HardwareObserved(value) => {
                convert(value).map_or(RxEvidence::Unavailable, RxEvidence::HardwareObserved)
            }
            Self::ProtocolValidated(value) => {
                convert(value).map_or(RxEvidence::Unavailable, RxEvidence::ProtocolValidated)
            }
            Self::Unavailable => RxEvidence::Unavailable,
        }
    }
}

/// Crypto result visible for an accepted receive MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxCryptoStatus {
    /// The received frame did not use link-layer encryption.
    Unprotected,
    /// The payload is plaintext and its integrity check has succeeded. Key
    /// identity and cipher negotiation remain upper-MAC security state.
    DecryptedAndIntegrityVerified,
}

/// The metadata a backend gives a received MPDU: the portable fields
/// ([`RxMeta`]), and whatever more the backend's type guarantees
/// ([`SignalMetadata`]).
pub trait RxMetadata: Copy {
    /// The portable fields.
    fn common(&self) -> &RxMeta;
}

impl RxMetadata for RxMeta {
    fn common(&self) -> &RxMeta {
        self
    }
}

/// A peer's signal over the noise floor in one received frame, in dB,
/// narrowed to a signed byte as the vendor's `ic_set_trc` does: what a rate
/// controller starts from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkMetric(i8);

impl LinkMetric {
    /// The metric of a frame received at `rssi_dbm` over `noise_floor_dbm`.
    pub const fn new(rssi_dbm: i8, noise_floor_dbm: i8) -> Self {
        Self(rssi_dbm.wrapping_sub(noise_floor_dbm))
    }

    /// The metric in dB.
    pub const fn db(self) -> i8 {
        self.0
    }
}

/// The metadata of a backend that measures every received frame's signal
/// and the noise floor it was received over: a property of the backend,
/// so code that needs the metric requires this trait and a backend
/// without it does not build with that code.
pub trait SignalMetadata: RxMetadata {
    /// The frame's signal over the noise floor.
    fn link_metric(&self) -> LinkMetric;
}

/// How a received MPDU was carried in its PPDU (IEEE Std 802.11-2020, 10.12,
/// A-MPDU operation). One state: an S-MPDU is carried in an A-MPDU too.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Aggregation {
    /// Not carried in an A-MPDU.
    Single,
    /// Carried in an A-MPDU and not an S-MPDU. This does not state how
    /// many MPDUs the A-MPDU carried. HT supplies an RXVECTOR Aggregation
    /// bit; the VHT and HE PPDU formats carry every MPDU in an A-MPDU.
    Ampdu,
    /// A VHT or HE S-MPDU: the sole MPDU of an A-MPDU, its delimiter's EOF
    /// bit set. Not a synonym for [`Aggregation::Single`].
    SMpdu,
}

/// Portable metadata of one received MPDU.
///
/// The backend publishes the physical fields it observed and leaves the
/// rest unavailable; protocol validation above the port may establish more.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxMeta {
    /// The channel the backend was configured to when it received the
    /// frame. This is configuration, not a per-frame observation: a
    /// receive report need carry no channel field the MAC can trust.
    pub channel: Channel,
    /// The received PPDU's decoded PHY rate, with its provenance.
    pub rate: RxEvidence<PhyRate>,
    /// The received signal level in dBm, with its provenance.
    pub rssi_dbm: RxEvidence<i8>,
    /// The receiver's noise floor when the frame arrived: a per-frame
    /// measurement, or the PHY's running estimate read at reception,
    /// observed either way.
    pub noise_floor_dbm: RxEvidence<i8>,
    /// Reception time in the port's radio clock, with the generation of
    /// the clock relation it was taken in.
    pub timestamp: RxEvidence<Ieee80211Stamp>,
    /// Encryption and integrity status, with its provenance.
    pub crypto: RxEvidence<RxCryptoStatus>,
    /// How the MPDU was carried in its PPDU, with its provenance.
    pub aggregation: RxEvidence<Aggregation>,
    /// Whether the MPDU carries an A-MSDU payload.
    pub amsdu: RxEvidence<bool>,
}

impl RxMeta {
    /// Metadata of a frame received on `channel` without any observation.
    pub const fn unavailable(channel: Channel) -> Self {
        Self {
            channel,
            rate: RxEvidence::Unavailable,
            rssi_dbm: RxEvidence::Unavailable,
            noise_floor_dbm: RxEvidence::Unavailable,
            timestamp: RxEvidence::Unavailable,
            crypto: RxEvidence::Unavailable,
            aggregation: RxEvidence::Unavailable,
            amsdu: RxEvidence::Unavailable,
        }
    }
}
