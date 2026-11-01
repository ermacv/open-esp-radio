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

use oer_ieee80211_contracts_port::{
    AggregateEnd, AggregateOutcome, Band, RetryPlan, RxMetadata, SingleOutcome, TxOutcome,
};
use oer_ieee80211_wire::phy::{LegacyRate, PhyRate};
use oer_time::Instant;

use crate::protection::BasicRates;
use crate::rate_control::{BlockAckTally, RatePeer};

/// Lowest management/group rate admitted by the band and BSS basic set.
///
/// Before a BSS is known, 2.4 GHz uses DSSS 1 Mb/s and 5 GHz uses OFDM
/// 6 Mb/s. A known set is authoritative: an empty or band-incompatible
/// set refuses transmission rather than introducing an unadvertised rate.
/// DSSS/CCK rates use the long preamble supplied by [`BasicRates`].
pub fn management_rate(band: Band, basic_rates: Option<&BasicRates>) -> Option<LegacyRate> {
    match basic_rates {
        None => Some(match band {
            Band::Ghz2_4 => LegacyRate::Dsss1M,
            Band::Ghz5 => LegacyRate::Ofdm6M,
        }),
        Some(rates) => rates
            .rates()
            .filter(|rate| band == Band::Ghz2_4 || rate.is_ofdm())
            .min_by_key(|rate| rate.kbps()),
    }
}

#[cfg(test)]
mod management_tests {
    use super::*;
    use oer_ieee80211_wire::phy::DsssPreamble;

    #[test]
    fn unknown_bss_uses_the_bands_lowest_rate() {
        assert_eq!(
            management_rate(Band::Ghz2_4, None),
            Some(LegacyRate::Dsss1M)
        );
        assert_eq!(management_rate(Band::Ghz5, None), Some(LegacyRate::Ofdm6M));
    }

    #[test]
    fn known_bss_uses_only_basic_rates_from_both_elements() {
        let basic = BasicRates::from_rate_elements(&[2, 0x84, 0x8b], &[0x8c, 0x98, 0xb0]);
        assert_eq!(
            management_rate(Band::Ghz2_4, Some(&basic)),
            Some(LegacyRate::Dsss2M(DsssPreamble::Long))
        );
        assert_eq!(
            management_rate(Band::Ghz5, Some(&basic)),
            Some(LegacyRate::Ofdm6M)
        );
    }

    #[test]
    fn ofdm_only_bss_never_introduces_one_megabit() {
        let basic = BasicRates::from_rate_elements(&[0x8c, 0x98, 0xb0], &[]);
        for band in [Band::Ghz2_4, Band::Ghz5] {
            assert_eq!(
                management_rate(band, Some(&basic)),
                Some(LegacyRate::Ofdm6M)
            );
        }
        let basic = BasicRates::from_rate_elements(&[0x98, 0xb0], &[]);
        assert_eq!(
            management_rate(Band::Ghz2_4, Some(&basic)),
            Some(LegacyRate::Ofdm12M)
        );
    }

    #[test]
    fn minimum_is_numeric_across_cck_and_ofdm() {
        let basic = BasicRates::from_rate_elements(&[0x96, 0x8c], &[]);
        assert_eq!(
            management_rate(Band::Ghz2_4, Some(&basic)),
            Some(LegacyRate::Ofdm6M)
        );
    }

    #[test]
    fn unusable_known_set_does_not_fall_back() {
        assert_eq!(
            management_rate(Band::Ghz2_4, Some(&BasicRates::EMPTY)),
            None
        );
        let dsss = BasicRates::from_rate_elements(&[0x82, 0x84], &[]);
        assert_eq!(management_rate(Band::Ghz5, Some(&dsss)), None);
    }
}

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
    /// An A-MPDU of the network's data: one publication at the first
    /// stage's rate.
    Aggregate,
}

/// The rates of one interface's frames, on a port whose received frames
/// carry the metadata `M`, whose solicited responses carry the evidence `A`
/// and whose aggregates' BlockAcks the evidence `BA`: a selection that needs
/// what `M`, `A` or `BA` does not guarantee (a signal measurement, an ACK's
/// SNR) is not one for that port.
pub trait RateSelection<M: RxMetadata, A, BA = A> {
    /// Packet-number step of this transmit policy, retained by each installed key.
    fn packet_number_step(&self) -> crate::ccmp::CcmpPacketNumberStep;

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

    /// Route one terminal's sample to the record of `frame`: only the
    /// completion of a `current` link's frame that carries a rate sample
    /// teaches the selection; a stale link's terminal or a frame that never
    /// went teaches nothing.
    fn learn<D>(
        &mut self,
        now: Instant,
        frame: FrameRates,
        current: bool,
        completion: &SingleOutcome<A, D>,
    ) {
        if current && completion.rate_sample().is_some() {
            self.completed(now, frame, completion);
        }
    }

    /// An aggregate of the network's data ended at `now` with `completion`;
    /// `tally` counts its subframes that went and those the BlockAck
    /// acknowledged (none without a BlockAck).
    fn completed_aggregate<D>(
        &mut self,
        now: Instant,
        tally: BlockAckTally,
        completion: &AggregateOutcome<BA, D>,
    );

    /// Route one aggregate terminal's sample: only a `current` link's
    /// aggregate that went on the air teaches the selection.
    fn learn_aggregate<D>(
        &mut self,
        now: Instant,
        current: bool,
        tally: BlockAckTally,
        completion: &AggregateOutcome<BA, D>,
    ) {
        let aired = matches!(
            completion,
            TxOutcome::Ended(
                AggregateEnd::Delivered { .. }
                    | AggregateEnd::AckTimeout(_)
                    | AggregateEnd::CtsTimeout(Some(_))
            )
        );
        if current && aired {
            self.completed_aggregate(now, tally, completion);
        }
    }
}

#[cfg(test)]
mod learn_tests {
    use super::*;
    use core::num::NonZeroU16;
    use oer_ieee80211_contracts_port::{
        Air, RxMeta, SingleEnd, SingleResponse, TxOutcome, WithdrawCause,
    };
    use std::vec::Vec;

    /// Records which record each taught completion reached.
    struct Taught(Vec<FrameRates>);

    impl RateSelection<RxMeta, ()> for Taught {
        fn packet_number_step(&self) -> crate::ccmp::CcmpPacketNumberStep {
            crate::ccmp::CcmpPacketNumberStep::ONE
        }

        fn plan<R>(&self, _: FrameRates, _: impl Fn(PhyRate) -> Option<R>) -> Option<RetryPlan<R>> {
            None
        }

        fn associated(&mut self, _: &RatePeer, _: &RxMeta) {}

        fn disconnected(&mut self) {}

        fn completed<D>(&mut self, _: Instant, frame: FrameRates, _: &SingleOutcome<(), D>) {
            self.0.push(frame);
        }

        fn completed_aggregate<D>(
            &mut self,
            _: Instant,
            _: BlockAckTally,
            _: &AggregateOutcome<(), D>,
        ) {
            self.0.push(FrameRates::Aggregate);
        }
    }

    #[test]
    fn only_a_current_links_aired_aggregate_teaches_the_selection() {
        let air = Air {
            transmissions: NonZeroU16::MIN,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        };
        let tally = BlockAckTally::new(NonZeroU16::MIN, 0).unwrap();
        let unanswered: AggregateOutcome<(), ()> = TxOutcome::Ended(AggregateEnd::AckTimeout(air));
        let unaired: AggregateOutcome<(), ()> =
            TxOutcome::Ended(AggregateEnd::ContentionLimit(None));
        let mut rates = Taught(Vec::new());
        rates.learn_aggregate(Instant::EPOCH, false, tally, &unanswered);
        rates.learn_aggregate(Instant::EPOCH, true, tally, &unaired);
        assert!(rates.0.is_empty());
        rates.learn_aggregate(Instant::EPOCH, true, tally, &unanswered);
        assert_eq!(rates.0, [FrameRates::Aggregate]);
    }

    #[test]
    fn only_a_current_links_sampled_completion_teaches_its_record() {
        let delivered: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::Delivered {
            air: Air {
                transmissions: NonZeroU16::MIN,
                rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
            },
            response: SingleResponse::Ack(()),
        });
        let withdrawn: SingleOutcome<(), ()> = TxOutcome::Withdrawn(WithdrawCause::GateClosed);
        let mut rates = Taught(Vec::new());
        let now = Instant::from_micros(1);
        rates.learn(now, FrameRates::Probe, true, &delivered);
        rates.learn(now, FrameRates::Data, false, &delivered);
        rates.learn(now, FrameRates::Control, true, &withdrawn);
        rates.learn(now, FrameRates::Data, true, &delivered);
        assert_eq!(rates.0, [FrameRates::Probe, FrameRates::Data]);
    }
}
