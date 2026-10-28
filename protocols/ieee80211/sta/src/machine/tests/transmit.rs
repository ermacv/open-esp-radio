//! The transmission parameters the station decides for each of its frames;
//! the service carries them into the port's attempt unchanged.

use super::*;
use oer_ieee80211_port::{PhyRate, Protection, RetryPlan, RetryStage, TxOperation};
use oer_ieee80211_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_ieee80211_wire::phy::LegacyRate;

/// A selection whose every record is two stages: 6 Mb/s ×15, then 1 Mb/s ×15.
struct TwoStages(core::cell::RefCell<Vec<FrameRates>>);

impl RateSelection<RxMeta, ()> for TwoStages {
    fn packet_number_step(&self) -> oer_ieee80211_upper_mac::ccmp::CcmpPacketNumberStep {
        oer_ieee80211_upper_mac::ccmp::CcmpPacketNumberStep::ONE
    }

    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        self.0.borrow_mut().push(frame);
        let stage = |rate| {
            Some(RetryStage {
                rate: carried(PhyRate::Legacy(rate))?,
                transmissions: core::num::NonZeroU8::new(15)?,
                protection: Protection::None,
            })
        };
        RetryPlan::new(stage(LegacyRate::Ofdm6M)?)
            .then(stage(LegacyRate::Dsss1M)?)
            .ok()
    }

    fn associated(&mut self, _: &oer_ieee80211_upper_mac::rate_control::RatePeer, _: &RxMeta) {}

    fn disconnected(&mut self) {}

    fn completed<D>(&mut self, _: Instant, _: FrameRates, _: &SingleOutcome<(), D>) {}
}

#[test]
fn each_plan_selects_its_record_and_a_null_keeps_its_own_transmissions() {
    let rates = TwoStages(core::cell::RefCell::new(Vec::new()));
    let management = StaRatePlan::Management
        .individual(&rates, Some)
        .expect("carried");
    assert_eq!(management.transmissions(), 30);
    let null = StaRatePlan::Null.individual(&rates, Some).expect("carried");
    assert_eq!(
        null.transmissions(),
        u16::from(crate::power_save::NULL_TRANSMISSIONS.get())
    );
    assert_eq!(
        StaRatePlan::group(&rates, Some),
        Some(PhyRate::Legacy(LegacyRate::Ofdm6M))
    );
    assert_eq!(
        *rates.0.borrow(),
        [FrameRates::Control, FrameRates::Control, FrameRates::Probe]
    );
    // A rate the phase does not carry refuses the frame.
    assert!(
        StaRatePlan::Management
            .individual(&rates, |_| None::<PhyRate>)
            .is_none()
    );
}

#[test]
fn station_frames_go_in_voice_unprotected_and_connection_frames_first() {
    type Transmit<'id> = StaTransmit<'id, Tokens<'id, 2>, Caps>;
    assert_eq!(Transmit::ACCESS_CATEGORY, WmmAccessCategory::Voice);
    assert_eq!(Transmit::PROTECTION, Protection::None);
    assert_eq!(
        StaFrameClass::Connection(ConnectionFrame::Eapol).coex_operation(),
        TxOperation::Connection(ConnectionFrame::Eapol)
    );
    assert_eq!(StaFrameClass::Normal.coex_operation(), TxOperation::Data);
}
