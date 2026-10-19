//! The Espressif station's transmit rate control: the per-association
//! schedule selection and its adaptation, recovered from the pinned
//! ESP32-S31 `libpp.a[trc.o]`.
//!
//! The controller walks the schedule arenas of [`crate::rate_schedule`]: it
//! selects the association's schedules (`rcUpdatePhyMode`), lowers the
//! ordinary schedule under retry pressure (`rcTxUpdatePer`), moves the
//! independent A-MPDU schedule from BlockAck windows
//! (`rcUpdateTxDoneAmpdu2`), filters ACK SNR (`rcUpdateAckSnr`) and selects
//! the beamforming report rate (`trc_set_bf_report_rate`). It holds values
//! only: a backend turns a schedule into its PHY rate and programs its own
//! report-rate registers.

use core::num::{NonZeroU16, NonZeroU32};

use crate::rate_schedule::{RateScheduleKind, RateScheduleRef, schedule_state};
use oer_base_time::Instant;
use oer_ieee80211_lower_mac::{BlockAckSnr, LinkMetric, SnrSample};
use oer_ieee80211_mac::{
    channel::WifiChannelWidth,
    he::{He20Capabilities, He20PeerState, HeDcmConstellation, HeMcsNssSupport},
    phy::{FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth},
    station::association::PhyMode,
};
use oer_ieee80211_upper_mac::rate_control::{
    AmpduUnavailable, BlockAckTally, MpduOutcome, NegotiatedLegacyRates, RateControl, RatePeer,
};

/// Instruction-evidenced fields of one 12-byte rate schedule record.
///
/// The remaining bytes select the actual PHY rate and retry sequence. They
/// are outside this reviewed projection; only the mutable schedule state used
/// by TX completion is owned here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateScheduleState {
    pub reference: RateScheduleRef,
    pub retry_limit: u8,
    pub adaptive: u8,
}

/// Rust-owned two-sample ACK-SNR filter used by transmit rate control.
///
/// Both bytes begin at the vendor sentinel `0x7f`. A sentinel input is not a
/// measurement and leaves the state unchanged. Keeping the bytes typed as
/// signed values makes the two different rounding rules explicit:
///
/// - the midpoint uses an arithmetic right shift, and therefore rounds a
///   negative odd sum toward negative infinity;
/// - the weighted average uses signed division, and therefore truncates
///   toward zero.
///
/// SOURCE(esp32s31): complete `libpp[trc.o]::rcUpdateAckSnr` (`0x42`
/// bytes). The body has no MMIO, callback or global-state access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AckSnrFilter {
    latest: i8,
    filtered: i8,
}

impl AckSnrFilter {
    pub const UNINITIALIZED: i8 = 0x7f;

    pub const fn new() -> Self {
        Self {
            latest: Self::UNINITIALIZED,
            filtered: Self::UNINITIALIZED,
        }
    }

    /// Consume one already decoded signed ACK-SNR sample.
    pub fn update(&mut self, sample: i8) {
        if sample == Self::UNINITIALIZED {
            return;
        }

        let midpoint = if self.latest == Self::UNINITIALIZED {
            0
        } else {
            (i16::from(self.latest).saturating_add(i16::from(sample)) >> 1) as i8
        };
        self.latest = sample;
        self.filtered = if self.filtered == Self::UNINITIALIZED {
            midpoint
        } else {
            (i16::from(self.filtered)
                .saturating_mul(3)
                .saturating_add(i16::from(midpoint))
                / 4) as i8
        };
    }

    pub const fn latest(self) -> Option<i8> {
        if self.latest == Self::UNINITIALIZED {
            None
        } else {
            Some(self.latest)
        }
    }

    pub const fn filtered(self) -> Option<i8> {
        if self.filtered == Self::UNINITIALIZED {
            None
        } else {
            Some(self.filtered)
        }
    }
}

impl Default for AckSnrFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Safe state mutated by the recovered `rcTxUpdatePer` transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RateControlState {
    pub retry_pressure: u8,
    pub weighted_retries: u32,
    pub transmissions: u32,
    pub completed: u32,
    pub reevaluate_after_us: u32,
    pub retry_state_1d: u8,
    pub retry_state_1e: u8,
    pub maximum_schedule_index: u8,
    pub current_schedule: RateScheduleState,
    pub legacy_schedule: RateScheduleRef,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleSelection {
    Unchanged,
    Selected(RateScheduleRef),
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxPerUpdate {
    pub schedule: ScheduleSelection,
}

/// Result of one Rust-owned A-MPDU BlockAck rate-control observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRateDecision {
    /// The vendor-sized observation window is not complete yet.
    Accumulating,
    /// One complete window was evaluated without changing its rate.
    Retain {
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
    /// Select the preceding (faster) record in the same schedule arena.
    Promote {
        from: RateScheduleRef,
        to: RateScheduleRef,
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
    /// Select the following (slower) record in the same schedule arena.
    Lower {
        from: RateScheduleRef,
        to: RateScheduleRef,
        raw_success_ratio: u8,
        filtered_success_ratio: u8,
    },
}

/// Independent rate state used by the vendor A-MPDU schedule getter.
///
/// This is intentionally not folded into [`RateControlState`]:
/// `rcGetAmpduSched` reads a separate rate byte, while `rcGetSched` reads the
/// ordinary current schedule pointer. Complete
/// `libpp.a[trc.o]::rcUpdateTxDoneAmpdu2` updates the former without
/// moving the latter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRateControlState {
    current_schedule: RateScheduleRef,
    highest_schedule_index: u8,
    attempted_mpdu: u32,
    acknowledged_mpdu: u32,
    last_evaluation_us: u32,
    last_lower_us: u32,
    last_promote_us: u32,
    promote_cooldown_us: u32,
    raw_success_ratio: u8,
    filtered_success_ratio: u8,
    evaluation_count: u32,
    promotion_probe: bool,
}

impl AmpduRateControlState {
    const COUNTER_RESCALE_LIMIT: u32 = 0x0200_0000;
    const EVALUATION_MPDU: u32 = 500;
    const EVALUATION_US: u32 = 100_000;
    const LOWER_HOLDOFF_US: u32 = 100_000;
    const INITIAL_PROMOTE_COOLDOWN_US: u32 = 500_000;
    const MAX_PROMOTE_COOLDOWN_US: u32 = 4_000_000;
    const VENDOR_AMPDU_FLOOR_INDEX: u8 = 8;

    fn new(
        schedule_kind: RateScheduleKind,
        ordinary_schedule_index: u8,
        highest_schedule_index: u8,
    ) -> Option<Self> {
        let ampdu_index = ordinary_schedule_index.min(Self::VENDOR_AMPDU_FLOOR_INDEX);
        Some(Self {
            current_schedule: RateScheduleRef::new(schedule_kind, ampdu_index)?,
            highest_schedule_index,
            attempted_mpdu: 0,
            acknowledged_mpdu: 0,
            last_evaluation_us: 0,
            last_lower_us: 0,
            last_promote_us: 0,
            promote_cooldown_us: Self::INITIAL_PROMOTE_COOLDOWN_US,
            raw_success_ratio: 0,
            filtered_success_ratio: 0,
            evaluation_count: 0,
            promotion_probe: false,
        })
    }

    pub const fn current_schedule(&self) -> RateScheduleRef {
        self.current_schedule
    }

    pub const fn raw_success_ratio(&self) -> Option<u8> {
        if self.evaluation_count == 0 {
            None
        } else {
            Some(self.raw_success_ratio)
        }
    }

    pub const fn filtered_success_ratio(&self) -> Option<u8> {
        if self.evaluation_count == 0 {
            None
        } else {
            Some(self.filtered_success_ratio)
        }
    }

    /// Consume one or more completed hardware A-MPDU attempts.
    ///
    /// Ratios use the blob's Q7 scale: 128 is a completely acknowledged
    /// window. `now_us` is the same wrapping 32-bit microsecond domain read by
    /// the vendor body from `0x2010d800`; the application may obtain it from
    /// its already-owned monotonic timer.
    ///
    /// This ports the complete scalar window/filter/adjacent-rate suffix at
    /// `rcUpdateTxDoneAmpdu2+0xb0..+0x2e4`. The earlier vendor-result layout
    /// decoder and the floor-index branch that also changes per-TID aggregate
    /// state remain outside this value API.
    pub fn observe_block_ack(
        &mut self,
        now_us: u32,
        tally: BlockAckTally,
        filtered_ack_snr: Option<i8>,
    ) -> AmpduRateDecision {
        let positive = NonZeroU32::from(tally.attempted());
        let acknowledged_mpdu = u32::from(tally.acknowledged());
        if self.acknowledged_mpdu.wrapping_add(acknowledged_mpdu) >= Self::COUNTER_RESCALE_LIMIT {
            self.attempted_mpdu >>= 1;
            self.acknowledged_mpdu >>= 1;
        }
        // Keep the input's nonzero witness through division. Accumulating
        // windows hold fewer than EVALUATION_MPDU (500) frames, and an input
        // holds at most u16::MAX: this sum cannot saturate a u32.
        let accumulated = positive.saturating_add(self.attempted_mpdu);
        self.attempted_mpdu = accumulated.get();
        self.acknowledged_mpdu = self.acknowledged_mpdu.wrapping_add(acknowledged_mpdu);

        if self.attempted_mpdu < Self::EVALUATION_MPDU
            && vendor_duration(now_us, self.last_evaluation_us) < Self::EVALUATION_US
        {
            return AmpduRateDecision::Accumulating;
        }

        let raw_success_ratio = ((self.acknowledged_mpdu << 7) / accumulated) as u8;
        let rate = schedule_state(self.current_schedule).rate;
        let ack_snr = filtered_ack_snr.unwrap_or(AckSnrFilter::UNINITIALIZED) as u8;
        let down_threshold = ampdu_down_threshold(rate, ack_snr);
        let previous_filtered = self.filtered_success_ratio;
        let filtered_success_ratio = if previous_filtered == 0 {
            let initial = (u16::from(down_threshold)
                .saturating_mul(3)
                .saturating_add(128)
                >> 2) as u8;
            if initial >= raw_success_ratio {
                initial
            } else {
                (u16::from(initial).saturating_add(u16::from(raw_success_ratio)) >> 1) as u8
            }
        } else {
            (u16::from(previous_filtered)
                .saturating_mul(3)
                .saturating_add(u16::from(raw_success_ratio))
                >> 2) as u8
        };

        self.last_evaluation_us = now_us;
        self.raw_success_ratio = raw_success_ratio;
        self.filtered_success_ratio = filtered_success_ratio;
        self.evaluation_count = self.evaluation_count.wrapping_add(1);
        self.attempted_mpdu = 0;
        self.acknowledged_mpdu = 0;

        if filtered_success_ratio < down_threshold && previous_filtered < down_threshold {
            if self.promotion_probe {
                self.promote_cooldown_us = self
                    .promote_cooldown_us
                    .saturating_mul(2)
                    .min(Self::MAX_PROMOTE_COOLDOWN_US);
            }
            if vendor_duration(now_us, self.last_lower_us) > Self::LOWER_HOLDOFF_US
                && self.current_schedule.index() < Self::VENDOR_AMPDU_FLOOR_INDEX
            {
                let from = self.current_schedule;
                if let Some(to) = from.advance() {
                    self.current_schedule = to;
                    self.last_lower_us = now_us;
                    self.clear_after_schedule_change();
                    return AmpduRateDecision::Lower {
                        from,
                        to,
                        raw_success_ratio,
                        filtered_success_ratio,
                    };
                }
            }
            return AmpduRateDecision::Retain {
                raw_success_ratio,
                filtered_success_ratio,
            };
        }

        if self.promotion_probe {
            self.promotion_probe = false;
            self.promote_cooldown_us = Self::INITIAL_PROMOTE_COOLDOWN_US;
            return AmpduRateDecision::Retain {
                raw_success_ratio,
                filtered_success_ratio,
            };
        }

        let up_threshold = ampdu_up_threshold(rate, ack_snr);
        if self.highest_schedule_index < self.current_schedule.index()
            && up_threshold < filtered_success_ratio
            && up_threshold < previous_filtered
            && vendor_duration(now_us, self.last_promote_us) > self.promote_cooldown_us
        {
            let from = self.current_schedule;
            if let Some(to) = RateScheduleRef::new(from.kind(), from.index().wrapping_sub(1)) {
                self.current_schedule = to;
                self.last_promote_us = now_us;
                self.promotion_probe = true;
                self.clear_after_schedule_change();
                return AmpduRateDecision::Promote {
                    from,
                    to,
                    raw_success_ratio,
                    filtered_success_ratio,
                };
            }
        }

        AmpduRateDecision::Retain {
            raw_success_ratio,
            filtered_success_ratio,
        }
    }

    fn clear_after_schedule_change(&mut self) {
        self.promote_cooldown_us = Self::INITIAL_PROMOTE_COOLDOWN_US;
        self.attempted_mpdu = 0;
        self.acknowledged_mpdu = 0;
        self.raw_success_ratio = 0;
        self.filtered_success_ratio = 0;
        self.evaluation_count = 0;
    }
}

const fn vendor_duration(now: u32, previous: u32) -> u32 {
    let duration = now.wrapping_sub(previous);
    if now < previous {
        duration.wrapping_sub(1)
    } else {
        duration
    }
}

const fn ampdu_rssi_margin(rate: u8, filtered_ack_snr: u8) -> u8 {
    let mcs = match rate {
        0x10..=0x19 => rate.saturating_sub(0x10),
        0x1a..=0x23 => rate.saturating_sub(0x1a),
        _ => return 0,
    };
    let reference = match mcs {
        0 => 8,
        1 => 11,
        2 => 13,
        3 => 16,
        4 => 21,
        5 => 26,
        6 => 29,
        7 => 33,
        8 => 36,
        _ => 41,
    };
    if filtered_ack_snr == AckSnrFilter::UNINITIALIZED as u8 || filtered_ack_snr <= reference {
        return 0;
    }
    let scaled =
        (((filtered_ack_snr.saturating_sub(reference)) >> 1) as u16).saturating_mul(3) as u8;
    if scaled > 32 { 32 } else { scaled }
}

const fn ampdu_up_threshold(rate: u8, filtered_ack_snr: u8) -> u8 {
    let margin = ampdu_rssi_margin(rate, filtered_ack_snr);
    match rate {
        0x10..=0x12 => 111_u8.saturating_sub(margin),
        0x13 => 116_u8.saturating_sub(margin),
        0x14..=0x19 | 0x21..=0x23 => 121_u8.saturating_sub(margin),
        _ => 121,
    }
}

const fn ampdu_down_threshold(rate: u8, filtered_ack_snr: u8) -> u8 {
    let margin = ampdu_rssi_margin(rate, filtered_ack_snr);
    match rate {
        0x10 => 105_u8.saturating_sub(margin),
        0x11 => 106_u8.saturating_sub(margin),
        0x12 => 107_u8.saturating_sub(margin),
        0x13 => 108_u8.saturating_sub(margin),
        0x14 => 109_u8.saturating_sub(margin),
        0x15 | 0x17 => 115_u8.saturating_sub(margin),
        0x16 | 0x18..=0x19 | 0x21..=0x23 => 114_u8.saturating_sub(margin),
        _ => 114,
    }
}

impl RateControlState {
    const COUNTER_RESCALE_LIMIT: u32 = 0x0200_0000;
    const REEVALUATE_AFTER_US: u32 = 500_000;

    /// Apply the complete scalar state transition from the pinned
    /// `libpp[trc.o]::rcTxUpdatePer` body.
    ///
    /// Arithmetic intentionally follows the RISC-V body: the per-result
    /// penalty is truncated to a byte before it is accumulated, retry pressure
    /// is a wrapping byte, and large counters are halved together.
    pub fn update_tx_per(&mut self, retries: u32) -> TxPerUpdate {
        let penalty = if u32::from(self.current_schedule.retry_limit) < retries {
            self.current_schedule.retry_limit.wrapping_add(2)
        } else {
            retries.wrapping_add(1) as u8
        };

        if self.transmissions.wrapping_add(1) >= Self::COUNTER_RESCALE_LIMIT {
            self.transmissions >>= 1;
            self.weighted_retries >>= 1;
        }
        self.transmissions = self.transmissions.wrapping_add(1);
        self.weighted_retries = self.weighted_retries.wrapping_add(u32::from(penalty));

        match retries {
            0..=2 => self.retry_pressure = 0,
            3..=4 => {}
            5..=7 => self.retry_pressure = self.retry_pressure.wrapping_add(1),
            _ => self.retry_pressure = self.retry_pressure.wrapping_add(2),
        }

        if self.retry_pressure <= 6 {
            return TxPerUpdate {
                schedule: ScheduleSelection::Unchanged,
            };
        }

        // Exact scalar part of `rcClearCurSched`.
        self.current_schedule.adaptive = 0;
        self.reevaluate_after_us = Self::REEVALUATE_AFTER_US;
        self.transmissions = 0;
        self.weighted_retries = 0;
        self.completed = 0;
        self.retry_state_1d = 0;
        self.retry_state_1e = 0;
        self.retry_pressure = 0;

        let next_index = u16::from(self.current_schedule.reference.index()) + 1;
        let selected = if u16::from(self.maximum_schedule_index) < next_index {
            self.legacy_schedule.offset(self.maximum_schedule_index)
        } else {
            self.current_schedule.reference.advance()
        };
        let schedule = match selected {
            Some(schedule) => ScheduleSelection::Selected(schedule),
            None => ScheduleSelection::Invalid,
        };
        TxPerUpdate { schedule }
    }
}

/// Hardware report-rate selection produced by the recovered link policy.
///
/// Fields are private so callers cannot construct a combination that the
/// complete blob policy never emits. Use [`beamforming_report_rate`] or
/// [`beamforming_report_rate_for_metric`] to obtain a value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeamformingReportRate {
    mode: u8,
    rate: u16,
    dcm: bool,
    ersu: bool,
    ersu_ack: bool,
}

impl BeamformingReportRate {
    pub const fn signal_mode(self) -> u8 {
        self.mode
    }

    pub const fn rate_code(self) -> u16 {
        self.rate
    }

    pub const fn dcm(self) -> bool {
        self.dcm
    }

    pub const fn extended_range_single_user(self) -> bool {
        self.ersu
    }

    pub const fn extended_range_ack(self) -> bool {
        self.ersu_ack
    }
}

/// Recovered PHY-family discriminator stored in a rate-control record, and
/// its rate-code callbacks: the Espressif family's policy data.
pub(crate) use crate::rate_schedule::RateIndexMap;
#[cfg(test)]
use crate::rate_schedule::rate_to_schedule_index;

/// Protocol-level PHY family used to create one associated STA rate context.
///
/// The blob has four internal HT/HE variants (numeric values 2 through 5),
/// but complete `rcUpdatePhyMode` applies the same schedule policy to all of
/// them. The open owner therefore does not expose those unowned numeric ABI
/// tags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaRateControlPhy {
    Dot11B,
    Dot11G,
    Ht,
    He,
    Lora,
}

/// Exact HE peer capabilities consumed by the low-link-metric report branch.
///
/// SOURCE(esp32s31): complete `libnet80211[wl_cnx.o]::ic_set_sta` copies
/// `!node[0x35c].bit(10)` and `node[0x348].bits(4:3)` into the scalar TRC
/// input. Complete `ieee80211_parse_heopr` names the former source
/// `ER-SU-Disable`; complete `ieee80211_parse_hecap` names the latter
/// `dcm rx constellation`. Complete
/// `libpp[if_hwctrl.o]::ic_set_trc` stores those values at vendor
/// record offsets `0x8f` and `0x90`, where `libpp[trc.o]::trc_set_bf_report_rate` tests
/// them for nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HeLowMetricReportFeatures {
    pub dcm_receive_supported: bool,
    pub extended_range_single_user_permitted: bool,
}

/// Highest peer rate supplied to the association-time vendor rate policy.
///
/// The scalar is deliberately private: the values consumed by
/// `rcGetHighestRateIdx` are data rates in half-megabit units, not PHY rate
/// bytes or MCS indices. Constructors keep that temporary blob convention out
/// of the application and make the negotiated capability family explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRateControlPeerHighestRate(u32);

impl StaRateControlPeerHighestRate {
    /// Construct the one-spatial-stream HE20 maximum selected from the peer's
    /// negotiated HE RX MCS/NSS map.
    ///
    /// A peer maximum above MCS 9 takes the MCS 9 value, the highest the
    /// recovered table encodes. The finite values below are the
    /// rounded half-megabit encodings accepted by complete
    /// `libpp[trc.o]::rcGetHighestRateIdx` and
    /// `libpp[trc.o]::rc11AXRate2SchedIdx`: `17,34,51,68,104,137,154,172,206,229`.
    pub const fn he20_one_spatial_stream(maximum_mcs: HeMcs) -> Self {
        Self(match maximum_mcs.index() {
            0 => 17,
            1 => 34,
            2 => 51,
            3 => 68,
            4 => 104,
            5 => 137,
            6 => 154,
            7 => 172,
            8 => 206,
            _ => 229,
        })
    }

    const fn vendor_half_mbps(self) -> u32 {
        self.0
    }
}

/// Value-only association input to the recovered per-peer rate policy.
///
/// [`StaLinkMetric`] keeps RSSI and noise floor from being confused with the
/// signed difference consumed by the schedule thresholds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaRateControlAssociationInput {
    pub phy: StaRateControlPhy,
    pub link_metric: StaLinkMetric,
    pub p2p: bool,
    pub peer_highest_rate: Option<StaRateControlPeerHighestRate>,
    /// Whether the peer's vendor LR rate list contains at least one local rate.
    ///
    /// Complete `libnet80211.a[ieee80211_phy.o]::
    /// ieee80211_setup_lr_rates` owns the count at node offset `0x84`;
    /// `ic_set_trc` copies it to record offset `0x8b`. It is not an HE
    /// capability or a generic request for more schedules.
    pub long_range_rates_present: bool,
    pub he_low_metric_report: HeLowMetricReportFeatures,
}

/// Signed `RSSI - noise floor` value consumed by the blob rate policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaLinkMetric(i8);

impl StaLinkMetric {
    /// Reproduce the narrowing subtraction performed by complete
    /// `libpp[if_hwctrl.o]::ic_set_trc` at instructions
    /// 0xca..0xce. The complete `wl_cnx.o::ic_set_sta` log string names the
    /// two source bytes `rssi` and `nf`.
    pub const fn from_rssi_and_noise_floor(rssi_dbm: i8, noise_floor_dbm: i8) -> Self {
        Self(rssi_dbm.wrapping_sub(noise_floor_dbm))
    }

    /// Admit a metric already produced by an owned running link estimator.
    pub const fn from_estimator(value: i8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i8 {
        self.0
    }
}

/// Rust-owned result of the vendor post-association rate-control transition.
///
/// SOURCE(esp32s31): complete `libnet80211[wl_cnx.o]::ic_set_sta`
/// constructs the per-peer input and calls
/// `libpp[if_hwctrl.o]::ic_set_trc`. Complete `ic_set_trc`
/// invokes `rcUpdatePhyMode` after copying only scalar peer state, and
/// complete `libpp[trc.o]::rcUpdatePhyMode` selects the schedule
/// references before calling `libpp[trc.o]::trc_set_bf_report_rate` at instruction 0x26c.
/// This owner preserves that value and hardware-programming order without the
/// vendor 0x98-byte record or its C offsets.
#[derive(Debug, Eq, PartialEq)]
pub struct StaRateControlAssociation {
    selection: PhyModeSelection,
    control_schedule: RateScheduleRef,
    beamforming_report: BeamformingReportRate,
    ack_snr: AckSnrFilter,
    runtime: RateControlState,
    ampdu_runtime: Option<AmpduRateControlState>,
}

impl StaRateControlAssociation {
    pub fn new(input: StaRateControlAssociationInput) -> Self {
        let (phy_type, he_type) = match input.phy {
            StaRateControlPhy::Dot11B => (0, 0),
            StaRateControlPhy::Dot11G => (1, 0),
            StaRateControlPhy::Ht => (2, 0),
            StaRateControlPhy::He => (2, 7),
            StaRateControlPhy::Lora => (6, 0),
        };
        let peer_highest_rate = input
            .peer_highest_rate
            .map_or(0, StaRateControlPeerHighestRate::vendor_half_mbps);
        let selection = select_phy_mode(PhyModeSelectionInput {
            phy_type,
            he_type,
            metric: i32::from(input.link_metric.value()),
            p2p: input.p2p,
            supplied_highest_rate: peer_highest_rate,
            use_supplied_highest_rate: input.peer_highest_rate.is_some(),
            long_range_rates_present: input.long_range_rates_present,
        });
        let beamforming_report = beamforming_report_rate_for_metric(
            i32::from(input.link_metric.value()),
            input.he_low_metric_report.dcm_receive_supported,
            input
                .he_low_metric_report
                .extended_range_single_user_permitted,
        );
        let current = schedule_state(selection.current);
        let ampdu_runtime = selection.ampdu_limit_rate.and_then(|_| {
            AmpduRateControlState::new(
                selection.current.kind(),
                selection.current.index(),
                selection.highest_index,
            )
        });
        Self {
            selection,
            control_schedule: control_schedule(input.phy, input.p2p),
            beamforming_report,
            ack_snr: AckSnrFilter::new(),
            runtime: RateControlState {
                retry_pressure: 0,
                weighted_retries: 0,
                transmissions: 0,
                completed: 0,
                reevaluate_after_us: 0,
                retry_state_1d: 0,
                retry_state_1e: 0,
                maximum_schedule_index: selection.maximum_index,
                current_schedule: RateScheduleState {
                    reference: selection.current,
                    retry_limit: current.retry_limit,
                    adaptive: current.adaptive,
                },
                legacy_schedule: selection.legacy,
            },
            ampdu_runtime,
        }
    }

    pub const fn current_schedule(&self) -> RateScheduleRef {
        self.runtime.current_schedule.reference
    }

    /// Schedule record of every non-data frame this association sends:
    /// management, BlockAck control, power-management Null and EAPOL.
    pub const fn control_schedule(&self) -> RateScheduleRef {
        self.control_schedule
    }

    pub const fn fallback_schedule(&self) -> RateScheduleRef {
        self.selection.fallback
    }

    pub const fn maximum_schedule_index(&self) -> u8 {
        self.selection.maximum_index
    }

    pub const fn schedule_count(&self) -> u8 {
        self.selection.schedule_count
    }

    pub const fn ampdu_limit_rate(&self) -> Option<u8> {
        self.selection.ampdu_limit_rate
    }

    pub const fn current_ampdu_schedule(&self) -> Option<RateScheduleRef> {
        match &self.ampdu_runtime {
            Some(runtime) => Some(runtime.current_schedule()),
            None => None,
        }
    }

    pub const fn beamforming_report(&self) -> BeamformingReportRate {
        self.beamforming_report
    }

    /// Update the running ACK-SNR estimate with one decoded successful result.
    ///
    /// The caller owns completion classification: failures have no valid ACK
    /// sample and must not call this method.
    pub fn update_ack_snr(&mut self, sample: i8) {
        self.ack_snr.update(sample);
    }

    pub const fn latest_ack_snr(&self) -> Option<i8> {
        self.ack_snr.latest()
    }

    pub const fn filtered_ack_snr(&self) -> Option<i8> {
        self.ack_snr.filtered()
    }

    /// Apply one completed exchange's retry count to the owned PER state.
    ///
    /// A schedule transition is installed as a complete typed record; no
    /// pointer or vendor-record offset escapes this owner.
    pub fn update_tx_per(&mut self, retries: u32) -> TxPerUpdate {
        let update = self.runtime.update_tx_per(retries);
        if let ScheduleSelection::Selected(reference) = update.schedule {
            let selected = schedule_state(reference);
            self.runtime.current_schedule = RateScheduleState {
                reference,
                retry_limit: selected.retry_limit,
                adaptive: selected.adaptive,
            };
        }
        update
    }

    /// Apply a checked aggregate observation, or return its unavailable
    /// controller before changing ACK-SNR or rate state.
    ///
    /// An unmeasured BlockAck leaves the ACK-SNR filter as it is, and the
    /// window and rate decision run on the current filtered SNR. This is a
    /// reviewed difference from the vendor.
    /// On a BlockAck whose hardware status is not success, the vendor still
    /// feeds the filter: `ppResortTxAMPDU` copies the descriptor, sets the
    /// copy's success byte `0x13` to 1 before each call (+0x10c, +0x26e) and
    /// keeps its ACK-SNR byte `0xd`; `rcUpdateTxDoneAmpdu2` passes
    /// `desc[0xd] + wDevCtrl[0x2e]` to `rcUpdateAckSnr` (+0x66..+0x7c). Only
    /// the success branch of `lmacProcessTxSuccess` writes a measurement into
    /// `0xd` (+0xe8); it and `lmacProcessTxopQComplete` write the sentinel
    /// 0x7f otherwise (+0x8e, +0x4c), and the error branches leave the byte
    /// untouched. The vendor sample is then the sentinel plus 0x60, −33 dB,
    /// which the filter accepts because its sentinel check runs before the
    /// addition, or a stale byte of an earlier transmission: no measurement.
    /// A failed aggregate still reaches the rate decision through its window.
    ///
    /// SOURCE(esp32s31): `libpp[pp.o]::ppResortTxAMPDU`,
    /// `libpp[trc.o]::rcUpdateTxDoneAmpdu2`, `libpp[lmac.o]::lmacProcessTxSuccess`,
    /// `libpp[lmac.o]::lmacProcessTxopQComplete` and `libpp[trc.o]::rcUpdateAckSnr`.
    pub fn observe_ampdu_block_ack(
        &mut self,
        now_us: u32,
        tally: BlockAckTally,
        ack: &impl BlockAckSnr,
    ) -> Result<AmpduRateDecision, AmpduUnavailable> {
        let Some(runtime) = &mut self.ampdu_runtime else {
            return Err(AmpduUnavailable);
        };
        match ack.snr_sample() {
            SnrSample::Measured(sample) => self.ack_snr.update(sample),
            SnrSample::Unmeasured => {}
        }
        let decision = runtime.observe_block_ack(now_us, tally, self.ack_snr.filtered());
        if matches!(
            decision,
            AmpduRateDecision::Promote { .. } | AmpduRateDecision::Lower { .. }
        ) {
            // Exact scalar side effect of `rcClearCurAMPDUSched`.
            self.runtime.current_schedule.adaptive = 0;
        }
        Ok(decision)
    }

    pub const fn ampdu_runtime(&self) -> Option<&AmpduRateControlState> {
        self.ampdu_runtime.as_ref()
    }

    pub const fn runtime(&self) -> &RateControlState {
        &self.runtime
    }
}

/// Value-only input to the recovered `rcUpdatePhyMode` policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyModeSelectionInput {
    pub phy_type: u8,
    pub he_type: u8,
    pub metric: i32,
    pub p2p: bool,
    pub supplied_highest_rate: u32,
    pub use_supplied_highest_rate: bool,
    pub long_range_rates_present: bool,
}

/// Complete scalar and schedule result of `rcUpdatePhyMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyModeSelection {
    pub current: RateScheduleRef,
    pub secondary: RateScheduleRef,
    pub fallback: RateScheduleRef,
    pub legacy: RateScheduleRef,
    pub highest_index: u8,
    pub maximum_index: u8,
    pub schedule_count: u8,
    pub index_map: RateIndexMap,
    /// The rate byte used to derive the initial AMPDU limit for HT/HE.
    pub ampdu_limit_rate: Option<u8>,
}

// Constant schedules are constructed only from literal compile-time values.
// The array equality proves the record bound without a runtime panic path.
macro_rules! literal_schedule {
    ($kind:expr, $index:expr) => {{
        const KIND: RateScheduleKind = $kind;
        const INDEX: u8 = $index;
        const _: [(); 1] = [(); ((INDEX as usize) < KIND.record_count()) as usize];
        const SCHEDULE: RateScheduleRef = match RateScheduleRef::new(KIND, INDEX) {
            Some(schedule) => schedule,
            None => panic!("a literal schedule is outside its arena"),
        };
        SCHEDULE
    }};
}

/// Non-data schedule of an interface whose association has not selected
/// one: `rc11BSchedTbl` record 3, 1 Mbit/s with a 32-publication budget.
///
/// SOURCE(esp32s31): `libpp[trc.o]::trc_init` stores this record at trc `+0x68`
/// (and `+0x64`, `+0x6c`) of the interface's default trc, which every frame
/// uses until `ic_set_sta` enables the association's own trc; station
/// authentication and association frames therefore use it.
pub const DEFAULT_CONTROL_SCHEDULE: RateScheduleRef =
    literal_schedule!(RateScheduleKind::Dot11B, 3);

/// The literal schedule records the selection names, checked at build.
const LORA_0: RateScheduleRef = literal_schedule!(RateScheduleKind::Lora, 0);
const LORA_1: RateScheduleRef = literal_schedule!(RateScheduleKind::Lora, 1);
const P2P_DOT11G_7: RateScheduleRef = literal_schedule!(RateScheduleKind::P2pDot11G, 7);
const BASIC_OFDM_0: RateScheduleRef = literal_schedule!(RateScheduleKind::BasicOfdm, 0);
const DOT11B_3: RateScheduleRef = DEFAULT_CONTROL_SCHEDULE;
const DOT11B_0: RateScheduleRef = literal_schedule!(RateScheduleKind::Dot11B, 0);

/// Non-data schedule `rcUpdatePhyMode` stores at trc `+0x68`.
///
/// `rcGetSched` selects it for every frame that is not ordinary data
/// (descriptor word 0 bit 3 clear or bit 25 set): management, BlockAck
/// control, power-management Null and EAPOL, whose classification sets bit 25.
///
/// SOURCE(esp32s31): `libpp[trc.o]::rcUpdatePhyMode` and `libpp[trc.o]::rcGetSched`, and
/// `libnet80211.a::ic_set_vif`, whose flag is set only by
/// `esp_wifi_config_11b_rate`: an 802.11b-only association keeps
/// `rc11BSchedTbl` record 3, 802.11g/n/ax use `BasicOFDMSched`, the flagged
/// 802.11g association uses `rcP2P11GSchedTbl` record 7 while flagged n/ax
/// keep the `trc_init` record, and LoRa uses `rcLoRaSchedTbl`.
const fn control_schedule(phy: StaRateControlPhy, flagged: bool) -> RateScheduleRef {
    match (phy, flagged) {
        (StaRateControlPhy::Dot11B, _) => DEFAULT_CONTROL_SCHEDULE,
        (StaRateControlPhy::Lora, _) => LORA_0,
        (StaRateControlPhy::Dot11G, true) => P2P_DOT11G_7,
        (StaRateControlPhy::Ht | StaRateControlPhy::He, true) => DEFAULT_CONTROL_SCHEDULE,
        (StaRateControlPhy::Dot11G | StaRateControlPhy::Ht | StaRateControlPhy::He, false) => {
            BASIC_OFDM_0
        }
    }
}

// Both arms are independently checked literals: a runtime family choice
// never creates a raw index that needs a fallback record.
macro_rules! g_schedule {
    ($p2p:expr, $index:expr) => {
        if $p2p {
            literal_schedule!(RateScheduleKind::P2pDot11G, $index)
        } else {
            literal_schedule!(RateScheduleKind::Dot11G, $index)
        }
    };
}

macro_rules! ht_schedule {
    ($he:expr, $index:expr) => {
        if $he {
            literal_schedule!(RateScheduleKind::Dot11Ax, $index)
        } else {
            literal_schedule!(RateScheduleKind::Dot11N, $index)
        }
    };
}

const fn highest_dot11b(rate: u32) -> RateScheduleRef {
    match rate {
        2 => literal_schedule!(RateScheduleKind::Dot11B, 3),
        4 => literal_schedule!(RateScheduleKind::Dot11B, 2),
        11 => literal_schedule!(RateScheduleKind::Dot11B, 1),
        _ => DOT11B_0,
    }
}

const fn highest_dot11g(rate: u32, p2p: bool) -> RateScheduleRef {
    match rate {
        12 => g_schedule!(p2p, 7),
        18 => g_schedule!(p2p, 6),
        24 => g_schedule!(p2p, 5),
        36 => g_schedule!(p2p, 4),
        48 => g_schedule!(p2p, 3),
        72 => g_schedule!(p2p, 2),
        96 => g_schedule!(p2p, 1),
        _ => g_schedule!(p2p, 0),
    }
}

const fn highest_dot11n(rate: u32) -> RateScheduleRef {
    match rate {
        13 => literal_schedule!(RateScheduleKind::Dot11N, 8),
        26 => literal_schedule!(RateScheduleKind::Dot11N, 7),
        39 => literal_schedule!(RateScheduleKind::Dot11N, 6),
        52 => literal_schedule!(RateScheduleKind::Dot11N, 5),
        78 => literal_schedule!(RateScheduleKind::Dot11N, 4),
        104 => literal_schedule!(RateScheduleKind::Dot11N, 3),
        117 => literal_schedule!(RateScheduleKind::Dot11N, 2),
        130 => literal_schedule!(RateScheduleKind::Dot11N, 1),
        _ => literal_schedule!(RateScheduleKind::Dot11N, 0),
    }
}

const fn highest_dot11ax(rate: u32) -> RateScheduleRef {
    match rate {
        17 => literal_schedule!(RateScheduleKind::Dot11Ax, 9),
        34 => literal_schedule!(RateScheduleKind::Dot11Ax, 8),
        51 => literal_schedule!(RateScheduleKind::Dot11Ax, 7),
        68 => literal_schedule!(RateScheduleKind::Dot11Ax, 6),
        104 => literal_schedule!(RateScheduleKind::Dot11Ax, 5),
        137 => literal_schedule!(RateScheduleKind::Dot11Ax, 4),
        154 => literal_schedule!(RateScheduleKind::Dot11Ax, 3),
        172 => literal_schedule!(RateScheduleKind::Dot11Ax, 2),
        206 => literal_schedule!(RateScheduleKind::Dot11Ax, 1),
        _ => literal_schedule!(RateScheduleKind::Dot11Ax, 0),
    }
}

/// The record selected by `rcGetHighestRateIdx` and its finite helpers.
/// Unknown wire rate encodings deliberately select the vendor's record zero;
/// the resulting reference is already valid for its selected arena.
const fn highest_rate_schedule(input: PhyModeSelectionInput) -> RateScheduleRef {
    if !input.use_supplied_highest_rate {
        return match input.phy_type {
            1 => g_schedule!(input.p2p, 0),
            2 | 4 => ht_schedule!(input.he_type == 7, 1),
            3 | 5 => ht_schedule!(input.he_type == 7, 0),
            6 => LORA_0,
            _ => DOT11B_0,
        };
    }
    match input.phy_type {
        0 => highest_dot11b(input.supplied_highest_rate),
        1 => highest_dot11g(input.supplied_highest_rate, input.p2p),
        2..=5 if input.he_type == 7 => highest_dot11ax(input.supplied_highest_rate),
        2..=5 => highest_dot11n(input.supplied_highest_rate),
        6 => LORA_0,
        _ => DOT11B_0,
    }
}

/// Compare the unadjusted HT threshold with the peer's highest record,
/// then select the checked HT record or its HE record two places lower.
const fn ht_metric_schedule(metric: i32, he: bool, highest: RateScheduleRef) -> RateScheduleRef {
    macro_rules! threshold {
        ($index:expr) => {
            if $index <= highest.index() {
                highest
            } else if he {
                literal_schedule!(RateScheduleKind::Dot11Ax, $index + 2)
            } else {
                literal_schedule!(RateScheduleKind::Dot11N, $index)
            }
        };
    }
    match metric {
        ..=8 => threshold!(11),
        9..=11 => threshold!(8),
        12..=13 => threshold!(7),
        14..=16 => threshold!(6),
        17..=21 => threshold!(5),
        22..=26 => threshold!(4),
        27..=29 => threshold!(3),
        30..=33 => threshold!(2),
        34..=36 => threshold!(1),
        _ => highest,
    }
}

/// Safe schedule selector recovered from `libpp[trc.o]::rcUpdatePhyMode`.
///
/// The result contains values only and is retained directly by the
/// Rust-owned association; no vendor record projection remains.
#[inline(never)]
pub(crate) fn select_phy_mode(input: PhyModeSelectionInput) -> PhyModeSelection {
    let highest = highest_rate_schedule(input);
    let lora_fallback = LORA_1;

    let (current, secondary, legacy, maximum_index, schedule_count, index_map, ampdu_limit_rate) =
        match input.phy_type {
            0 => {
                let current = if input.use_supplied_highest_rate {
                    highest
                } else {
                    DOT11B_3
                };
                (
                    current,
                    DOT11B_3,
                    DOT11B_0,
                    if input.long_range_rates_present { 5 } else { 3 },
                    6,
                    RateIndexMap::Dot11B,
                    None,
                )
            }
            1 => {
                let current = if input.use_supplied_highest_rate {
                    highest
                } else if input.metric <= 11 {
                    if input.p2p {
                        P2P_DOT11G_7
                    } else {
                        literal_schedule!(RateScheduleKind::Dot11G, 10)
                    }
                } else if input.metric <= 16 {
                    g_schedule!(input.p2p, 5)
                } else if input.metric <= 21 {
                    g_schedule!(input.p2p, 3)
                } else if input.metric < 30 {
                    g_schedule!(input.p2p, 2)
                } else {
                    g_schedule!(input.p2p, 0)
                };
                (
                    current,
                    if input.p2p {
                        P2P_DOT11G_7
                    } else {
                        BASIC_OFDM_0
                    },
                    g_schedule!(input.p2p, 0),
                    if input.p2p {
                        7
                    } else if input.long_range_rates_present {
                        12
                    } else {
                        10
                    },
                    if input.p2p { 8 } else { 13 },
                    RateIndexMap::Dot11G,
                    None,
                )
            }
            2..=5 => {
                let he = input.he_type == 7;
                let metric_schedule = ht_metric_schedule(input.metric, he, highest);
                let current = if input.use_supplied_highest_rate {
                    highest
                } else if input.p2p && metric_schedule.index() > 7 {
                    ht_schedule!(he, 7)
                } else {
                    metric_schedule
                };
                let limit_schedule = if current.index() > 8 {
                    ht_schedule!(he, 8)
                } else {
                    current
                };
                (
                    current,
                    if input.p2p {
                        P2P_DOT11G_7
                    } else {
                        BASIC_OFDM_0
                    },
                    ht_schedule!(he, 0),
                    if input.long_range_rates_present {
                        if he { 15 } else { 13 }
                    } else if he {
                        13
                    } else {
                        11
                    },
                    if he { 16 } else { 14 },
                    if he {
                        RateIndexMap::Dot11Ax
                    } else {
                        RateIndexMap::Dot11N
                    },
                    Some(schedule_state(limit_schedule).rate),
                )
            }
            6 => (LORA_0, LORA_0, LORA_0, 1, 2, RateIndexMap::Lora, None),
            _ => (
                DOT11B_3,
                DOT11B_3,
                DOT11B_0,
                if input.long_range_rates_present { 5 } else { 3 },
                6,
                RateIndexMap::Dot11B,
                None,
            ),
        };

    PhyModeSelection {
        current,
        secondary,
        fallback: if input.long_range_rates_present {
            lora_fallback
        } else {
            secondary
        },
        legacy,
        highest_index: highest.index(),
        maximum_index,
        schedule_count,
        index_map,
        ampdu_limit_rate,
    }
}

/// Safe policy recovered from complete `trc_set_bf_report_rate`.
///
/// The byte subtraction and signed narrowing intentionally match the RISC-V
/// instructions used by the blob.
pub const fn beamforming_report_rate(
    filtered_ack_snr: u8,
    quarter_noise_floor: i32,
    he_feature_8f: bool,
    he_feature_90: bool,
) -> BeamformingReportRate {
    let metric = filtered_ack_snr.wrapping_sub(quarter_noise_floor as u8) as i8;
    beamforming_report_rate_for_metric(metric as i32, he_feature_8f, he_feature_90)
}

/// Direct form used by `rcLowerSched`, `rcUpSched` and the recovered
/// `rcUpdatePhyMode` branch, whose callers already supply the signed link
/// metric rather than ACK SNR and noise-floor components.
pub const fn beamforming_report_rate_for_metric(
    metric: i32,
    he_feature_8f: bool,
    he_feature_90: bool,
) -> BeamformingReportRate {
    if metric > 13 {
        BeamformingReportRate {
            mode: 1,
            rate: 16,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    } else if he_feature_8f && he_feature_90 {
        BeamformingReportRate {
            mode: 2,
            rate: 16,
            dcm: true,
            ersu: true,
            ersu_ack: true,
        }
    } else {
        BeamformingReportRate {
            mode: 0,
            rate: 11,
            dcm: false,
            ersu: false,
            ersu_ack: false,
        }
    }
}

/// The Espressif controller behind the rate-control seam: the vendor's
/// per-association rate control, its schedules decoded as portable rates
/// within what the link negotiated. It was recovered from the vendor
/// station; an access point keeps one per associated station with it, a
/// use of ours.
///
/// A schedule's HT rate takes the link's width, and its short guard
/// interval only where the peer supports one at that width; an HE rate
/// takes 0.8 us with one HE-LTF where the peer supports it, two otherwise,
/// and LDPC where the peer receives it. A schedule
/// without a portable rate (the vendor's Long Range modes) sends at the
/// vendor station's fallback: 54 Mb/s legacy, or HT MCS 7 with the long
/// guard interval.
pub struct EspressifRateControl {
    association: StaRateControlAssociation,
    decode: RateDecode,
}

/// What turns a schedule into a rate the association can send.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RateDecode {
    legacy_rates: NegotiatedLegacyRates,
    high_throughput: bool,
    ht_bandwidth: PpduBandwidth,
    ht_short_gi: bool,
    he_800ns_gi_ltf: HeGiLtf,
    he_ldpc: bool,
}

impl RateDecode {
    fn rate(self, schedule: RateScheduleRef) -> PhyRate {
        let code = schedule_state(schedule).rate;
        match crate::rate_code::phy_rate(
            schedule.kind(),
            code,
            self.ht_bandwidth,
            self.he_800ns_gi_ltf,
        ) {
            // A schedule rate the peer's capabilities cannot carry as is
            // goes at the vendor station's fallback, as one without a
            // portable rate does.
            Some(PhyRate::Ht(rate)) if rate.short_gi() && !self.ht_short_gi => {
                HtRate::new(rate.mcs(), rate.bandwidth(), false)
                    .map_or_else(|| self.fallback(), PhyRate::Ht)
            }
            Some(PhyRate::He(rate)) if self.he_ldpc => HeRate::new(
                rate.mcs(),
                rate.spatial_streams(),
                rate.bandwidth(),
                rate.gi_ltf(),
                FecCoding::Ldpc,
                rate.dcm(),
            )
            .map_or_else(|| self.fallback(), PhyRate::He),
            Some(PhyRate::Legacy(rate)) => PhyRate::Legacy(self.legacy_rates.constrain(rate)),
            Some(rate) => rate,
            None => self.fallback(),
        }
    }

    /// The vendor station's fallback: HT MCS 7 with the long guard interval
    /// on an HT association, at its width, or the highest negotiated legacy
    /// rate no faster than 54 Mb/s.
    fn fallback(self) -> PhyRate {
        match HtRate::new(HtMcs::SINGLE_STREAM_MAX, self.ht_bandwidth, false) {
            Some(rate) if self.high_throughput => PhyRate::Ht(rate),
            _ => PhyRate::Legacy(self.legacy_rates.constrain(LegacyRate::Ofdm54M)),
        }
    }
}

impl RateControl for EspressifRateControl {
    type Config = ();

    fn for_peer((): (), peer: &RatePeer, link_metric: LinkMetric) -> Self {
        let he = peer.he_capabilities;
        let phy = match peer.phy {
            PhyMode::Legacy if !peer.legacy_rates.has_ofdm() => StaRateControlPhy::Dot11B,
            PhyMode::Legacy => StaRateControlPhy::Dot11G,
            PhyMode::Ht20 | PhyMode::Ht40 => StaRateControlPhy::Ht,
            PhyMode::He20 => StaRateControlPhy::He,
        };
        // As the vendor station: an HE peer's maximum one-stream MCS, capped
        // at MCS 9.
        let peer_highest_rate = he
            .filter(|_| peer.phy == PhyMode::He20)
            .and_then(|capability| match capability.receive_nss1 {
                HeMcsNssSupport::Mcs0To7 => HeMcs::new(7),
                HeMcsNssSupport::Mcs0To9 | HeMcsNssSupport::Mcs0To11 => HeMcs::new(9),
                HeMcsNssSupport::NotSupported => None,
            })
            .map(StaRateControlPeerHighestRate::he20_one_spatial_stream);
        let association = StaRateControlAssociation::new(StaRateControlAssociationInput {
            phy,
            link_metric: StaLinkMetric::from_estimator(link_metric.db()),
            p2p: false,
            peer_highest_rate,
            long_range_rates_present: false,
            he_low_metric_report: HeLowMetricReportFeatures {
                dcm_receive_supported: he.is_some_and(|capability| {
                    capability.dcm_receive_constellation() != HeDcmConstellation::NotSupported
                }),
                extended_range_single_user_permitted: peer
                    .he_peer_state
                    .is_some_and(He20PeerState::extended_range_single_user_permitted),
            },
        });
        let (ht_bandwidth, width) = if peer.phy == PhyMode::Ht40 {
            (PpduBandwidth::Mhz40, WifiChannelWidth::Mhz40Above)
        } else {
            (PpduBandwidth::Mhz20, WifiChannelWidth::Mhz20)
        };
        Self {
            association,
            decode: RateDecode {
                legacy_rates: peer.legacy_rates,
                high_throughput: peer.phy != PhyMode::Legacy,
                ht_bandwidth,
                ht_short_gi: peer
                    .ht_capabilities
                    .is_some_and(|capability| capability.supports_short_guard_interval(width)),
                he_800ns_gi_ltf: if he.is_some_and(He20Capabilities::supports_one_ltf_800ns_gi) {
                    HeGiLtf::Ltf1xGi800Ns
                } else {
                    HeGiLtf::Ltf2xGi800Ns
                },
                he_ldpc: he.is_some_and(He20Capabilities::supports_ldpc_coding_in_payload),
            },
        }
    }

    fn mpdu_rate(&self) -> PhyRate {
        self.decode.rate(self.association.current_schedule())
    }

    fn ampdu_rate(&self) -> PhyRate {
        self.decode.rate(
            self.association
                .current_ampdu_schedule()
                .unwrap_or_else(|| self.association.current_schedule()),
        )
    }

    fn observe_mpdu(&mut self, attempts: NonZeroU16, outcome: MpduOutcome, ack_snr_db: Option<i8>) {
        if outcome == MpduOutcome::Acknowledged
            && let Some(sample) = ack_snr_db
        {
            self.association.update_ack_snr(sample);
        }
        let retries = attempts.get() - 1;
        self.association.update_tx_per(u32::from(retries));
    }

    fn observe_ampdu<B: BlockAckSnr>(
        &mut self,
        now: Instant,
        tally: BlockAckTally,
        ack: &B,
    ) -> Result<(), AmpduUnavailable> {
        self.association
            .observe_ampdu_block_ack(vendor_micros(now), tally, ack)
            .map(|_| ())
    }
}

/// `now` in the vendor's wrapping 32-bit microsecond domain: its low 32
/// bits, which the A-MPDU window state compares by wrapping difference.
fn vendor_micros(now: Instant) -> u32 {
    let [b0, b1, b2, b3, ..] = now.as_micros().to_le_bytes();
    u32::from_le_bytes([b0, b1, b2, b3])
}

impl EspressifRateControl {
    /// Apply association admission to a control or retry schedule's rate.
    pub(super) fn constrain_rate(&self, rate: PhyRate) -> PhyRate {
        match rate {
            PhyRate::Legacy(rate) => PhyRate::Legacy(self.decode.legacy_rates.constrain(rate)),
            rate => rate,
        }
    }

    /// The association's rate control, as the vendor holds it.
    pub const fn association(&self) -> &StaRateControlAssociation {
        &self.association
    }
}

#[cfg(test)]
mod tests;
