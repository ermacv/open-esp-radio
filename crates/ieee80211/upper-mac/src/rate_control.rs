//! The choice of data rates to one peer, and what its exchanges teach it.
//!
//! Which rate a data frame is first sent at is decided above the lower-MAC
//! port, for every role alike. [`RateControl`] is the seam: a station keeps
//! one controller for its access point, an access point one per associated
//! station; each asks its controller for the rate of a single MPDU and of an
//! A-MPDU and reports every exchange's outcome to it. The controller is an
//! integrator's policy: [`FixedRateControl`] keeps one rate; the Espressif
//! controller of `oer-espressif-ieee80211-policy::rate_control` adapts as the
//! vendor does.

use core::num::NonZeroU16;

use oer_ieee80211_lower_mac::{BlockAckSnr, LinkMetric};
use oer_ieee80211_mac::{
    he::{He20Capabilities, He20PeerState},
    ht::HtPeerCapabilities,
    phy::PhyRate,
    station::association::PhyMode,
};
use oer_time::Instant;

mod legacy;
pub use legacy::NegotiatedLegacyRates;

/// What a controller knows of the peer it sends to: the PHY mode of the
/// link and the peer's receive capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RatePeer {
    /// The mode the link runs in: the peers' common format and width.
    pub phy: PhyMode,
    /// Every supported non-HT rate of the association, including non-basic
    /// rates and the Extended Supported Rates element. Control and retry
    /// fallback must remain within this nonempty set even on an HT/HE link.
    pub legacy_rates: NegotiatedLegacyRates,
    pub ht_capabilities: Option<HtPeerCapabilities>,
    pub he_capabilities: Option<He20Capabilities>,
    /// The HE state of an HE link.
    pub he_peer_state: Option<He20PeerState>,
}

/// Whether the peer acknowledged an MPDU whose transmission count is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduOutcome {
    /// The expected ACK arrived.
    Acknowledged,
    /// The exchange ended without the expected ACK.
    Unacknowledged,
}

/// A BlockAck observation of at least one attempted MPDU.
///
/// Acknowledgements cannot exceed attempts. Only [`Self::new`] constructs
/// this proof, before a controller's rate or SNR state changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockAckTally {
    attempted: NonZeroU16,
    acknowledged: u16,
}

/// A BlockAck count that exceeds the number of attempted MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExcessAcknowledgements {
    /// The nonzero number of attempted MPDUs.
    pub attempted: NonZeroU16,
    /// The impossible number of acknowledgements.
    pub acknowledged: u16,
}

impl BlockAckTally {
    /// Validate the counts of one completed aggregate observation.
    pub const fn new(
        attempted: NonZeroU16,
        acknowledged: u16,
    ) -> Result<Self, ExcessAcknowledgements> {
        if acknowledged > attempted.get() {
            return Err(ExcessAcknowledgements {
                attempted,
                acknowledged,
            });
        }
        Ok(Self {
            attempted,
            acknowledged,
        })
    }

    /// How many MPDUs were attempted, including unacknowledged ones.
    pub const fn attempted(self) -> NonZeroU16 {
        self.attempted
    }

    /// How many of the attempted MPDUs were acknowledged.
    pub const fn acknowledged(self) -> u16 {
        self.acknowledged
    }
}

/// This peer's controller has no A-MPDU state to observe an exchange with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduUnavailable;

/// The rate control of one peer.
///
/// A zero-count observation cannot reach a controller:
///
/// ```compile_fail,E0061
/// use oer_ieee80211_upper_mac::rate_control::RateControl;
/// fn empty<C: RateControl, B: oer_ieee80211_lower_mac::BlockAckSnr>(
///     control: &mut C,
///     now: oer_time::Instant,
///     ack: &B,
/// ) {
///     control.observe_ampdu(now, 0, 0, ack);
/// }
/// ```
///
/// Single-frame observations also require a nonzero transmission count:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_upper_mac::rate_control::RateControl;
/// fn empty<C: RateControl>(control: &mut C) {
///     control.observe_mpdu(0, false, None);
/// }
/// ```
///
/// A caller carries checked counts and an explicit MPDU outcome:
///
/// ```
/// use core::num::NonZeroU16;
/// use oer_ieee80211_lower_mac::{BlockAckSnr, SnrSample};
/// use oer_ieee80211_upper_mac::rate_control::{
///     AmpduUnavailable, BlockAckTally, MpduOutcome, RateControl,
/// };
/// struct Measured(i8);
/// impl BlockAckSnr for Measured {
///     fn snr_sample(&self) -> SnrSample {
///         SnrSample::Measured(self.0)
///     }
/// }
/// fn observe<C: RateControl>(control: &mut C, now: oer_time::Instant)
///     -> Result<(), AmpduUnavailable>
/// {
///     control.observe_mpdu(NonZeroU16::MIN, MpduOutcome::Acknowledged, Some(30));
///     let tally = BlockAckTally::new(NonZeroU16::MIN, 1).unwrap();
///     control.observe_ampdu(now, tally, &Measured(30))
/// }
/// ```
pub trait RateControl: Sized {
    /// What the integrator configures every peer's controller with.
    type Config: Copy;

    /// The controller of `peer`, whose signal over the noise floor was
    /// `link_metric` in the frame that made the link (the station's
    /// Association Response, or the access point's Association Request).
    fn for_peer(config: Self::Config, peer: &RatePeer, link_metric: LinkMetric) -> Self;

    /// The rate of a data MPDU's first attempt sent alone.
    fn mpdu_rate(&self) -> PhyRate;

    /// The rate of an A-MPDU's first attempt.
    fn ampdu_rate(&self) -> PhyRate;

    /// A single data MPDU's exchange ended after `attempts` known MPDU transmissions,
    /// with `outcome` and the ACK SNR of a valid sample.
    fn observe_mpdu(&mut self, attempts: NonZeroU16, outcome: MpduOutcome, ack_snr_db: Option<i8>);

    /// An A-MPDU exchange ended at `now` with checked counts and the
    /// backend's evidence `ack` of its BlockAck, which splits into an SNR
    /// sample or none: an unmeasured BlockAck feeds no SNR. A controller
    /// without A-MPDU state returns [`AmpduUnavailable`] without changing
    /// rate or SNR state; the caller receives this refusal directly.
    fn observe_ampdu<B: BlockAckSnr>(
        &mut self,
        now: Instant,
        tally: BlockAckTally,
        ack: &B,
    ) -> Result<(), AmpduUnavailable>;
}

/// Every data frame at one configured rate, whatever its exchanges show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedRateControl(PhyRate);

impl RateControl for FixedRateControl {
    type Config = PhyRate;

    fn for_peer(rate: PhyRate, _peer: &RatePeer, _link_metric: LinkMetric) -> Self {
        Self(rate)
    }

    fn mpdu_rate(&self) -> PhyRate {
        self.0
    }

    fn ampdu_rate(&self) -> PhyRate {
        self.0
    }

    fn observe_mpdu(
        &mut self,
        _attempts: NonZeroU16,
        _outcome: MpduOutcome,
        _ack_snr_db: Option<i8>,
    ) {
    }

    fn observe_ampdu<B: BlockAckSnr>(
        &mut self,
        _now: Instant,
        _tally: BlockAckTally,
        _ack: &B,
    ) -> Result<(), AmpduUnavailable> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_mac::phy::LegacyRate;

    use super::*;

    #[test]
    fn a_fixed_controller_keeps_its_rate() {
        let rate = PhyRate::Legacy(LegacyRate::Ofdm24M);
        let peer = RatePeer {
            phy: PhyMode::Legacy,
            legacy_rates: NegotiatedLegacyRates::from_rate_elements(
                &[2, 4, 11, 22, 12, 18, 24, 36],
                &[48, 72, 96, 108],
            )
            .unwrap(),
            ht_capabilities: None,
            he_capabilities: None,
            he_peer_state: None,
        };
        let mut control = FixedRateControl::for_peer(rate, &peer, LinkMetric::new(-40, -50));
        control.observe_mpdu(
            NonZeroU16::new(7).unwrap(),
            MpduOutcome::Unacknowledged,
            None,
        );
        assert_eq!((control.mpdu_rate(), control.ampdu_rate()), (rate, rate));
    }

    #[test]
    fn block_ack_counts_are_checked_before_the_controller_observes_them() {
        for attempts in [
            NonZeroU16::MIN,
            NonZeroU16::new(64).unwrap(),
            NonZeroU16::MAX,
        ] {
            for acknowledged in [0, attempts.get()] {
                let tally = BlockAckTally::new(attempts, acknowledged).unwrap();
                assert_eq!(tally.attempted(), attempts);
                assert_eq!(tally.acknowledged(), acknowledged);
            }
        }
        assert_eq!(
            BlockAckTally::new(NonZeroU16::MIN, 2),
            Err(ExcessAcknowledgements {
                attempted: NonZeroU16::MIN,
                acknowledged: 2
            })
        );
    }
}
