//! The rates of every frame one interface sends: a sans-IO machine above
//! the lower-MAC port.
//!
//! The service asks it for the retry plan of each frame by what the frame
//! is for ([`FrameRates`]) and hands it every terminal of those frames
//! ([`RateSelection::completed`]): the rates and transmissions the attempt
//! made teach it the next plan. It hears when its interface associates
//! with a peer and when it leaves: a controller adapts per peer. The selection is an integrator's policy;
//! the Espressif one (`oer-espressif-ieee80211-policy::rate_selection`)
//! sends as the vendor's station does.

use oer_ieee80211_lower_mac::{RetryPlan, RxMetadata, SingleOutcome};
use oer_ieee80211_mac::phy::PhyRate;
use oer_time::Instant;

use crate::rate_control::RatePeer;

/// What a frame is for, which picks its rates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameRates {
    /// A group Probe Request: one transmission, the first stage's rate.
    Probe,
    /// An individual frame that carries no network data: management,
    /// EAPOL, a Null, a BlockAckReq.
    Control,
    /// A data MPDU of the network, sent alone.
    Data,
}

/// The rates of one interface's frames, on a port whose received frames
/// carry the metadata `M` and whose solicited responses carry the evidence
/// `A`: a selection that needs what `M` or `A` does not guarantee (a signal
/// measurement, an ACK's SNR) is not one for that port.
pub trait RateSelection<M: RxMetadata, A> {
    /// The retry plan of the next frame for `frame`, in the rates the
    /// port's channel carries: `carried` turns a rate into the phase's, or
    /// refuses it. The selection's own rule decides what a refused stage
    /// becomes; `None` when no stage is left, and the frame does not go.
    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>>;

    /// The interface associated with `peer`, whose frame that made the link
    /// (the station's Association Response) arrived with `response`: from
    /// now on its frames go at the association's rates.
    fn associated(&mut self, peer: &RatePeer, response: &M);

    /// The interface left its peer.
    fn disconnected(&mut self);

    /// A frame for `frame` ended at `now` with `completion`: its
    /// transmissions, the rate of the last, whether it was acknowledged.
    fn completed<D>(&mut self, now: Instant, frame: FrameRates, completion: &SingleOutcome<A, D>);
}
