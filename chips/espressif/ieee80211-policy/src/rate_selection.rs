//! The Espressif rate selection: the rates of a station's frames as the
//! vendor's station sends them.
//!
//! - A group Probe Request goes once at 1 Mb/s.
//! - Every other frame without network data (management, EAPOL, Nulls,
//!   BlockAckReqs) walks a control schedule record: the interface's
//!   default one ([`DEFAULT_CONTROL_SCHEDULE`], 1 Mb/s DSSS) until the
//!   station associated, then the association's
//!   ([`StaRateControlAssociation::control_schedule`](crate::rate_control::StaRateControlAssociation::control_schedule)),
//!   the four-way handshake's EAPOL included. The rates depend on what is
//!   known of the access point, which the association makes known; the
//!   vendor switches only once connected, a consequence of when it calls
//!   `ic_set_sta` (a deliberate departure).
//! - A data MPDU starts at the rate the association's rate control holds
//!   ([`EspressifRateControl`]) and steps down the vendor's retry ladder
//!   ([`ordinary_retry_rate`]), within the publication budget of the
//!   current schedule record; its terminal adapts the control.
//!
//! A record is four `(rate, count)` pairs, so each plan holds at most
//! four stages ([`oer_ieee80211_contracts_port::RETRY_STAGES`]); once the ladder has no further rate,
//! the remaining transmissions keep the last one, as the vendor's retry
//! state does.
//!
//! The records were recovered from the ESP32-S31, which has no 5 GHz
//! band, where 1 Mb/s DSSS is not allowed: a selection builds only for a
//! port without 5 GHz, checked at compile time, until the records of a
//! 5 GHz chip are recovered.

use crate::rate_record::RateScheduleRef;
use core::marker::PhantomData;
use core::num::NonZeroU8;

use oer_ieee80211_contracts_port::{
    AckSnr, Band, Ieee80211Capabilities, Protection, RateSample, RetryPlan, RetryStage,
    SignalMetadata, SingleOutcome,
};
use oer_ieee80211_core_upper_mac::{
    rate_control::{MpduOutcome, RateControl, RatePeer},
    rate_selection::{FrameRates, RateSelection},
};
use oer_ieee80211_wire::phy::{LegacyRate, PhyRate};
use oer_time::Instant;

use crate::{
    rate_code::legacy_rate,
    rate_control::{DEFAULT_CONTROL_SCHEDULE, EspressifRateControl},
    rate_schedule::{schedule_publication_limit, schedule_rate_after_failures},
    retry_ladder::ordinary_retry_rate,
};

/// The Espressif rate selection of one station interface on a port of the
/// capabilities `K`.
///
/// A port with a 5 GHz band builds none, nor one without the rates the
/// vendor's records name: DSSS/CCK, OFDM, HT MCS 0-7 and HE MCS 0-9 with
/// LDPC.
///
/// ```compile_fail,E0080
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_contracts_port::*;
/// # use oer_ieee80211_wire::phy::{HeMcs, HtMcs};
/// # const fn caps(bands: BandSet) -> LowerMacCapabilities {
/// #     LowerMacCapabilities {
/// #         bands, widths: WidthSet::MHZ20,
/// #         rates: RateSupport { dsss_cck: true, ofdm: true, ht_max_mcs: HtMcs::new(7),
/// #             he_max_mcs: HeMcs::new(9), he_max_bandwidth_mhz: 20, he_dcm: false,
/// #             he_ldpc: true, spatial_streams: 1 },
/// #         services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #         tx_power_ceiling_min_dbm: None,
/// #         individual_no_ack: PhyFormatSet::NON_HT,
/// #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #         edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #         edca_max_txop_limit_units_32_us: u16::MAX,
/// #     }
/// # }
/// struct DualBand;
/// impl Ieee80211Capabilities for DualBand {
///     type SppScope = oer_ieee80211_wire::security::spp::Association;
///     const CAPS: LowerMacCapabilities = caps(BandSet::GHZ2_4.union(BandSet::GHZ5));
/// }
/// let selection = EspressifRateSelection::<DualBand>::new();
/// # core::hint::black_box(selection);
/// ```
///
/// The same port without its 5 GHz band builds one:
///
/// ```
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_contracts_port::*;
/// # use oer_ieee80211_wire::phy::{HeMcs, HtMcs};
/// # const fn caps(bands: BandSet) -> LowerMacCapabilities {
/// #     LowerMacCapabilities {
/// #         bands, widths: WidthSet::MHZ20,
/// #         rates: RateSupport { dsss_cck: true, ofdm: true, ht_max_mcs: HtMcs::new(7),
/// #             he_max_mcs: HeMcs::new(9), he_max_bandwidth_mhz: 20, he_dcm: false,
/// #             he_ldpc: true, spatial_streams: 1 },
/// #         services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #         tx_power_ceiling_min_dbm: None,
/// #         individual_no_ack: PhyFormatSet::NON_HT,
/// #         station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #         rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #         edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #         edca_max_txop_limit_units_32_us: u16::MAX,
/// #     }
/// # }
/// struct SingleBand;
/// impl Ieee80211Capabilities for SingleBand {
///     type SppScope = oer_ieee80211_wire::security::spp::Association;
///     const CAPS: LowerMacCapabilities = caps(BandSet::GHZ2_4);
/// }
/// let selection = EspressifRateSelection::<SingleBand>::new();
/// # core::hint::black_box(selection);
/// ```
pub struct EspressifRateSelection<K> {
    /// The rate control of the associated peer.
    control: Option<EspressifRateControl>,
    port: PhantomData<fn() -> K>,
}

/// What a port must carry for the vendor's records.
struct Port<K>(PhantomData<fn() -> K>);

impl<K: Ieee80211Capabilities> Port<K> {
    const CHECKED: () = {
        if K::CAPS.bands.contains(Band::Ghz5) {
            panic!("the vendor's rate records are recovered for 2.4 GHz only");
        }
        let rates = K::CAPS.rates;
        if !(rates.dsss_cck && rates.ofdm) {
            panic!("the vendor's records name DSSS/CCK and OFDM rates");
        }
        if !matches!(rates.ht_max_mcs, Some(mcs) if mcs.index() >= 7) {
            panic!("the vendor's records name HT MCS 0-7");
        }
        if !(matches!(rates.he_max_mcs, Some(mcs) if mcs.index() >= 9) && rates.he_ldpc) {
            panic!("the vendor's records name HE MCS 0-9 with LDPC");
        }
    };
}

impl<K: Ieee80211Capabilities> EspressifRateSelection<K> {
    /// The selection of an interface that is not connected.
    pub const fn new() -> Self {
        let () = Port::<K>::CHECKED;
        Self {
            control: None,
            port: PhantomData,
        }
    }

    /// The rate control of the connected peer.
    pub const fn control(&self) -> Option<&EspressifRateControl> {
        self.control.as_ref()
    }

    /// The control schedule record in use.
    fn control_schedule(&self) -> RateScheduleRef {
        match &self.control {
            Some(control) => control.association().control_schedule(),
            None => DEFAULT_CONTROL_SCHEDULE,
        }
    }
}

impl<K: Ieee80211Capabilities> Default for EspressifRateSelection<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The plan of `limit` transmissions, the first at `initial` and each
/// after `failures` failures at `rate(failures)`, the last rate where it
/// has none. A record holds four `(rate, count)` pairs, so its walk makes
/// at most four stages. A source with more rate changes refuses the complete
/// plan rather than publishing a smaller transmission budget.
fn walk(
    initial: PhyRate,
    limit: u8,
    rate: impl Fn(u8) -> Option<PhyRate>,
) -> Option<RetryPlan<PhyRate>> {
    let stage = |(rate, transmissions): (PhyRate, NonZeroU8)| RetryStage {
        rate,
        transmissions,
        protection: Protection::None,
    };
    let limit = NonZeroU8::new(limit)?;
    let mut current = (initial, NonZeroU8::MIN);
    let mut plan: Option<RetryPlan<PhyRate>> = None;
    for failures in 1..limit.get() {
        let next = rate(failures).unwrap_or(current.0);
        if next == current.0 {
            // A run is bounded by the byte-sized publication limit; its
            // nonzero count cannot exceed 255, including this increment.
            current.1 = current.1.saturating_add(1);
        } else {
            plan = Some(match plan {
                None => RetryPlan::new(stage(current)),
                Some(plan) => match plan.then(stage(current)) {
                    Ok(plan) => plan,
                    Err(_) => return None,
                },
            });
            current = (next, NonZeroU8::MIN);
        }
    }
    match plan {
        None => Some(RetryPlan::new(stage(current))),
        Some(plan) => plan.then(stage(current)).ok(),
    }
}

/// The plan of a control schedule record: its rates, its budget; `None`
/// for a record whose first rate is no legacy rate (none the station uses:
/// a test pins it), and the frame does not go.
fn control_plan(
    schedule: RateScheduleRef,
    constrain: impl Fn(PhyRate) -> PhyRate,
) -> Option<RetryPlan<PhyRate>> {
    let rate = |failures| {
        schedule_rate_after_failures(schedule, failures)
            .and_then(legacy_rate)
            .map(PhyRate::Legacy)
            .map(&constrain)
    };
    walk(rate(0)?, schedule_publication_limit(schedule), rate)
}

/// The selection for a port whose metadata measures every received frame's
/// signal ([`SignalMetadata`]) and whose ACK evidence carries the
/// response's SNR ([`AckSnr`]): the association's rate control starts from
/// the Association Response's signal over the noise floor and filters the
/// ACK SNR as the vendor's `rcUpdateAckSnr` does. A port whose metadata does
/// not measure the signal has no Espressif selection (the `rate-selection`
/// UI suite holds the port without ACK SNR):
///
/// ```compile_fail,E0277
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_contracts_port::*;
/// # use oer_ieee80211_core_upper_mac::rate_selection::RateSelection;
/// # struct Caps;
/// # impl Ieee80211Capabilities for Caps { type SppScope = oer_ieee80211_wire::security::spp::Association; const CAPS: LowerMacCapabilities = PORT; }
/// # const PORT: LowerMacCapabilities = LowerMacCapabilities {
/// #     bands: BandSet::GHZ2_4, widths: WidthSet::MHZ20,
/// #     rates: RateSupport { dsss_cck: true, ofdm: true,
/// #         ht_max_mcs: oer_ieee80211_wire::phy::HtMcs::new(7),
/// #         he_max_mcs: oer_ieee80211_wire::phy::HeMcs::new(9), he_max_bandwidth_mhz: 20,
/// #         he_dcm: false, he_ldpc: true, spatial_streams: 1 },
/// #     services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #     tx_power_ceiling_min_dbm: None,
/// #     individual_no_ack: PhyFormatSet::NON_HT,
/// #     station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #     edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #     edca_max_txop_limit_units_32_us: u16::MAX,
/// # };
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// # struct Snr(i8);
/// # impl AckSnr for Snr { fn snr_db(&self) -> i8 { self.0 } }
/// fn for_port<M: RxMetadata>(selection: impl RateSelection<M, Snr>) {}
/// // The portable metadata measures nothing.
/// for_port::<RxMeta>(EspressifRateSelection::<Caps>::new());
/// ```
///
/// Metadata that measures the signal takes one:
///
/// ```
/// # use oer_espressif_ieee80211_policy::rate_selection::EspressifRateSelection;
/// # use oer_ieee80211_contracts_port::*;
/// # use oer_ieee80211_core_upper_mac::rate_selection::RateSelection;
/// # struct Caps;
/// # impl Ieee80211Capabilities for Caps { type SppScope = oer_ieee80211_wire::security::spp::Association; const CAPS: LowerMacCapabilities = PORT; }
/// # const PORT: LowerMacCapabilities = LowerMacCapabilities {
/// #     bands: BandSet::GHZ2_4, widths: WidthSet::MHZ20,
/// #     rates: RateSupport { dsss_cck: true, ofdm: true,
/// #         ht_max_mcs: oer_ieee80211_wire::phy::HtMcs::new(7),
/// #         he_max_mcs: oer_ieee80211_wire::phy::HeMcs::new(9), he_max_bandwidth_mhz: 20,
/// #         he_dcm: false, he_ldpc: true, spatial_streams: 1 },
/// #     services: HardwareServices::FCS, vifs: 1, tx_queues: 4, max_mpdu_length: 2304,
/// #     tx_power_ceiling_min_dbm: None,
/// #     individual_no_ack: PhyFormatSet::NON_HT,
/// #     station_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
/// #     rx_block_ack_max_tid: 0, rx_block_ack_max_window: 0,
/// #     edca_max_aifsn: 15, edca_max_cw_exponent: 10,
/// #     edca_max_txop_limit_units_32_us: u16::MAX,
/// # };
/// #[derive(Clone, Copy)]
/// struct Measured {
///     common: RxMeta,
///     rssi_dbm: i8,
///     noise_floor_dbm: i8,
/// }
/// impl RxMetadata for Measured {
///     fn common(&self) -> &RxMeta {
///         &self.common
///     }
/// }
/// impl SignalMetadata for Measured {
///     fn link_metric(&self) -> LinkMetric {
///         LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
///     }
/// }
/// #[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// struct Snr(i8);
/// impl AckSnr for Snr {
///     fn snr_db(&self) -> i8 {
///         self.0
///     }
/// }
/// fn for_port<M: RxMetadata>(selection: impl RateSelection<M, Snr>) {}
/// for_port::<Measured>(EspressifRateSelection::<Caps>::new());
/// ```
impl<K: Ieee80211Capabilities, M: SignalMetadata, A: AckSnr> RateSelection<M, A>
    for EspressifRateSelection<K>
{
    fn packet_number_step(&self) -> oer_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep {
        crate::ccmp::TX_PACKET_NUMBER_STEP
    }

    fn plan<R>(
        &self,
        frame: FrameRates,
        carried: impl Fn(PhyRate) -> Option<R>,
    ) -> Option<RetryPlan<R>> {
        on_channel(self.vendor_plan(frame)?, carried)
    }

    fn associated(&mut self, peer: &RatePeer, response: &M) {
        self.control = Some(EspressifRateControl::for_peer(
            (),
            peer,
            response.link_metric(),
        ));
    }

    fn disconnected(&mut self) {
        self.control = None;
    }

    fn completed<D>(&mut self, _now: Instant, frame: FrameRates, completion: &SingleOutcome<A, D>) {
        if frame != FrameRates::Data {
            return;
        }
        let (transmissions, outcome, ack_snr_db) = match completion.rate_sample() {
            Some(RateSample::Acked { air, ack }) => (
                air.transmissions,
                MpduOutcome::Acknowledged,
                Some(ack.snr_db()),
            ),
            // A frame that solicited no response went out, which is all it
            // asked for; it carries no ACK SNR.
            Some(RateSample::Unsolicited(air)) => {
                (air.transmissions, MpduOutcome::Acknowledged, None)
            }
            Some(RateSample::Unanswered(air)) => {
                (air.transmissions, MpduOutcome::Unacknowledged, None)
            }
            None => return,
        };
        if let Some(control) = &mut self.control {
            control.observe_mpdu(transmissions, outcome, ack_snr_db);
        }
    }
}

impl<K: Ieee80211Capabilities> EspressifRateSelection<K> {
    /// The vendor's plan for `frame`, before the channel's rule.
    fn vendor_plan(&self, frame: FrameRates) -> Option<RetryPlan<PhyRate>> {
        match (frame, &self.control) {
            // The vendor station's probe: once, at 1 Mb/s.
            (FrameRates::Probe, _) => Some(RetryPlan::new(RetryStage {
                rate: PhyRate::Legacy(LegacyRate::Dsss1M),
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            })),
            (FrameRates::Data, Some(control)) => {
                let initial = control.mpdu_rate();
                walk(
                    initial,
                    schedule_publication_limit(control.association().current_schedule()),
                    |failures| {
                        ordinary_retry_rate(initial, failures)
                            .map(|rate| control.constrain_rate(rate))
                    },
                )
            }
            // No data goes before an association; it would go as control.
            (FrameRates::Control | FrameRates::Data, control) => {
                control_plan(self.control_schedule(), |rate| {
                    control
                        .as_ref()
                        .map_or(rate, |control| control.constrain_rate(rate))
                })
            }
        }
    }
}

/// The vendor's plan on the port's channel: a stage whose rate the channel
/// does not carry (an HT40 rate on a 20 MHz channel) is left out with its
/// transmissions, and the plan is what remains in order; `None` when no
/// stage is left. The vendor's station never meets this: its records are
/// for its own channel, and the rule only keeps a plan within the port's.
fn on_channel<R>(
    plan: RetryPlan<PhyRate>,
    carried: impl Fn(PhyRate) -> Option<R>,
) -> Option<RetryPlan<R>> {
    plan.filter_map(carried)
}

#[cfg(test)]
mod tests;
