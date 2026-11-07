//! Time on air: what an exchange takes and whose turn the channel gives.
//!
//! Without [`AirConfig`] the model answers a publication at once. With it
//! ([`ModelAir::set_air_time`](crate::ModelAir::set_air_time)) a publication
//! waits for its exchange on one shared channel, an access point sends the
//! other way, and frames land in the receive ring while its PPDU is on the
//! air.
//!
//! # Durations
//!
//! Every duration comes from [`PhyRate::max_ppdu_duration_micros`], the
//! production TXTIME, and the delimiters and padding of an A-MPDU from
//! [`oer_protocols_ieee80211_wire::ampdu`]. Only control-frame lengths, SIFS,
//! the slot and the backoff rules are written here, each with its source.
//! An [`AirExchange`] is the protection, the PPDU and its response; the
//! access before it depends on the channel.
//!
//! # The channel
//!
//! [`Medium`] holds one transmission at a time: the station's head request,
//! the access point's aggregate or a [`BusyChannel`] burst. A side waits AIFS
//! and its backoff, drawn from a seed, once the channel is idle, and freezes
//! the remaining slots while another side holds it. A side that ended a
//! transmission draws its next backoff at once, with or without another
//! frame. Two sides whose access falls on one instant collide: nobody hears
//! either, each doubles its contention window and the channel stays busy
//! for the longer. A burst defers to a transmission on the channel and to
//! nothing else, and no side's backoff runs through it.
//!
//! # What stays an input
//!
//! The rate of the response, the access point's aggregation rule and limits
//! ([`AirPeer`]), its window while the production recipient cannot agree one
//! ([`PeerWindow::Assumed`]), and how long the channel is busy for others.
//! The frames of the access point land when the ring has a slot and only
//! those are acknowledged, as the recipient's scoreboard does in hardware:
//! an assumption about the chip, not an observation of it.
//!
//! The consumer's credit, the units a caller holds, the ring and the Block
//! Ack window are separate limits ([`ModelRxLimits`], [`PeerWindow`]), so a
//! scenario can tell which one binds.
//!
//! # What it leaves out
//!
//! It predicts no CPU: every software turn takes no time. No frame is lost
//! to noise unless an [`crate::AirAnswer`] scripts it, a TXOP holds one
//! exchange, the station's queues contend as one side and the channel has
//! exactly the sides named here. An attempt that ends before its data
//! reaches the air (a missing CTS, a denial, a collision or hardware timeout
//! the test scripts) takes no channel time, its RTS included.

use crate::ModelRxMeta;
use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
use core::num::NonZeroU32;
use oer_protocols_ieee80211_contracts_port::{PhyRate, Protection, Tid};
use oer_protocols_ieee80211_wire::ampdu;
use oer_protocols_ieee80211_wire::extensions::wmm::{WmmAcParameters, WmmParameterSet};
use oer_protocols_ieee80211_wire::phy::LegacyRate;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The octets of an ACK or a CTS, FCS included.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1.3 and 9.3.1.4.
pub const ACK_OCTETS: u32 = 14;
/// The octets of an RTS, FCS included.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1.2.
pub const RTS_OCTETS: u32 = 20;
/// The octets of a compressed BlockAck, FCS included.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.1.8.4.
pub const BLOCK_ACK_OCTETS: u32 = 32;
/// The octets an FCS adds to an MPDU.
pub const FCS_OCTETS: u32 = 4;
/// The microseconds of one unit of a TXOP limit.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.28.
pub const TXOP_UNIT_MICROS: u32 = 32;

/// The channel's interframe times and the rate of its responses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirTiming {
    /// aSIFSTime.
    pub sifs_micros: u32,
    /// aSlotTime.
    pub slot_micros: u32,
    /// The rate an ACK and a BlockAck go at: the peer's choice, an input.
    pub response_rate: LegacyRate,
}

impl AirTiming {
    /// HT at 2.4 GHz with the short slot, responses at 24 Mb/s: the highest
    /// mandatory ERP-OFDM rate at or below an HT PPDU's.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 19.4.4 (aSIFSTime 10 us, aSlotTime 9 us)
    /// and 10.6.6.5.2 (the rate of a control response frame).
    pub const HT_2_4_GHZ: Self = Self {
        sifs_micros: 10,
        slot_micros: 9,
        response_rate: LegacyRate::Ofdm24M,
    };
}

/// What an air rule could not compute: the model refuses the publication's
/// time rather than making one up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirTimeError {
    /// A duration does not fit its integer.
    Unrepresentable,
    /// A frame has no length.
    EmptyFrame,
    /// One frame of the access point exceeds its own limits.
    PeerFrameOutsideLimits,
    /// An event came due and nothing happened: the medium broke its own
    /// rules, and stops rather than spin.
    Stalled,
}

/// What answers a PPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirResponse {
    /// Nothing.
    None,
    /// An ACK after SIFS.
    Ack,
    /// A compressed BlockAck after SIFS.
    BlockAck,
}

/// The time of one exchange, once a side holds the channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirExchange {
    /// RTS, CTS and their SIFS, or the CTS-to-self and its SIFS.
    pub protection_micros: u32,
    /// The PPDU.
    pub ppdu_micros: u32,
    /// SIFS and the response.
    pub response_micros: u32,
    /// How long the exchange keeps the channel when it collides: an RTS
    /// loses its RTS and CTS time, any other frame its own PPDU.
    lost_micros: u32,
}

impl AirExchange {
    /// The exchange of a PSDU of `psdu_octets` at `rate`.
    ///
    /// # Errors
    ///
    /// [`AirTimeError::Unrepresentable`] for a duration past `u32`.
    pub fn new(
        timing: AirTiming,
        rate: PhyRate,
        psdu_octets: u32,
        protection: Protection,
        response: AirResponse,
    ) -> Result<Self, AirTimeError> {
        let frame = |legacy: LegacyRate, octets: u32| {
            PhyRate::Legacy(legacy)
                .max_ppdu_duration_micros(octets)
                .ok_or(AirTimeError::Unrepresentable)
        };
        let sum = |parts: &[u32]| {
            parts
                .iter()
                .try_fold(0_u32, |total, part| total.checked_add(*part))
                .ok_or(AirTimeError::Unrepresentable)
        };
        let sifs = timing.sifs_micros;
        let ppdu_micros = rate
            .max_ppdu_duration_micros(psdu_octets)
            .ok_or(AirTimeError::Unrepresentable)?;
        let response_micros = match response {
            AirResponse::None => 0,
            AirResponse::Ack => sum(&[sifs, frame(timing.response_rate, ACK_OCTETS)?])?,
            AirResponse::BlockAck => sum(&[sifs, frame(timing.response_rate, BLOCK_ACK_OCTETS)?])?,
        };
        let (protection_micros, lost_micros) = match protection {
            Protection::None => (0, ppdu_micros),
            Protection::RtsCts { rate } => {
                let (rts, cts) = (frame(rate, RTS_OCTETS)?, frame(rate, ACK_OCTETS)?);
                (sum(&[rts, sifs, cts, sifs])?, sum(&[rts, sifs, cts])?)
            }
            Protection::CtsToSelf { rate } => {
                let protection = sum(&[frame(rate, ACK_OCTETS)?, sifs])?;
                (protection, sum(&[protection, ppdu_micros])?)
            }
        };
        Ok(Self {
            protection_micros,
            ppdu_micros,
            response_micros,
            lost_micros,
        })
    }

    /// How long the exchange keeps the channel when it is answered.
    ///
    /// # Errors
    ///
    /// [`AirTimeError::Unrepresentable`] for a duration past `u32`.
    pub fn occupancy_micros(&self) -> Result<u32, AirTimeError> {
        self.protection_micros
            .checked_add(self.ppdu_micros)
            .and_then(|micros| micros.checked_add(self.response_micros))
            .ok_or(AirTimeError::Unrepresentable)
    }
}

/// The PSDU of an A-MPDU of `mpdus`, each with its FCS and MIC, as
/// [`ampdu::length_after_append`] lays it out under the start spacing of
/// `spacing`, the IEEE encoding.
///
/// # Errors
///
/// [`AirTimeError::EmptyFrame`] for an empty MPDU or an empty aggregate,
/// [`AirTimeError::Unrepresentable`] for a rate without A-MPDUs or an
/// overflow.
pub fn ampdu_psdu_octets(
    mpdus: impl IntoIterator<Item = u32>,
    rate: PhyRate,
    spacing: u8,
) -> Result<u32, AirTimeError> {
    let mut run = PsduRun::new(rate, spacing)?;
    for mpdu in mpdus {
        run.append(mpdu)?;
    }
    if run.length == 0 {
        return Err(AirTimeError::EmptyFrame);
    }
    Ok(run.length)
}

/// An A-MPDU's length as its MPDUs join it.
#[derive(Clone, Copy)]
struct PsduRun {
    length: u32,
    previous: Option<(u32, u32)>,
    minimum: u32,
}

impl PsduRun {
    fn new(rate: PhyRate, spacing: u8) -> Result<Self, AirTimeError> {
        let minimum = rate
            .minimum_mpdu_spacing_bytes(spacing)
            .ok_or(AirTimeError::Unrepresentable)?;
        Ok(Self {
            length: 0,
            previous: None,
            minimum,
        })
    }

    fn after(&self, mpdu: u32) -> Result<Self, AirTimeError> {
        if mpdu == 0 {
            return Err(AirTimeError::EmptyFrame);
        }
        let length = ampdu::length_after_append(self.length, self.previous, mpdu)
            .ok_or(AirTimeError::Unrepresentable)?;
        let delimiters =
            ampdu::empty_delimiters(mpdu, self.minimum).ok_or(AirTimeError::Unrepresentable)?;
        Ok(Self {
            length,
            previous: Some((mpdu, delimiters)),
            minimum: self.minimum,
        })
    }

    fn append(&mut self, mpdu: u32) -> Result<(), AirTimeError> {
        *self = self.after(mpdu)?;
        Ok(())
    }
}

/// How one side contends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirAccess {
    /// AIFS.
    pub aifs_micros: u32,
    /// CWmin.
    pub cw_min: u32,
    /// CWmax.
    pub cw_max: u32,
}

impl AirAccess {
    /// AIFS = SIFS + AIFSN * aSlotTime, CW = 2^ECW - 1.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 10.23.2.4 and 9.4.2.28.
    #[must_use]
    pub fn of(timing: AirTiming, parameters: WmmAcParameters) -> Self {
        let window = |exponent: u8| {
            1_u32
                .checked_shl(u32::from(exponent))
                .unwrap_or(u32::MAX)
                .saturating_sub(1)
        };
        Self {
            aifs_micros: timing.sifs_micros.saturating_add(
                u32::from(parameters.aifsn.get()).saturating_mul(timing.slot_micros),
            ),
            cw_min: window(parameters.ecw_min.get()),
            cw_max: window(parameters.ecw_max.get()),
        }
    }
}

/// How much the access point offers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Offered {
    /// Its queue never empties.
    Saturated,
    /// This many frames arrive per second, evenly.
    FramesPerSecond(NonZeroU32),
}

/// The Block Ack window the access point's aggregates stay in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerWindow {
    /// The agreement the station's owner applied through the port for
    /// [`AirPeer::tid`]; single MPDUs without one.
    Agreed,
    /// This window whatever the port applied: a recipient the production
    /// owner cannot agree, for a scenario that asks what it would change.
    Assumed(u16),
}

/// What the access point sends the station.
pub struct AirPeer {
    /// The MPDU of a Sequence Number, as the receive ring takes it.
    pub frame: Box<dyn FnMut(SequenceNumber) -> Vec<u8>>,
    /// What the hardware reports of each frame.
    pub meta: ModelRxMeta,
    /// What each frame carries on the air beside that image: its FCS and
    /// the CCMP header and MIC the ring's image does not show.
    pub air_overhead_octets: u32,
    /// The rate of its PPDUs.
    pub rate: PhyRate,
    /// How much it offers.
    pub offered: Offered,
    /// The window its aggregates stay in.
    pub window: PeerWindow,
    /// The TID of its frames.
    pub tid: Tid,
    /// Its contention and TXOP limit.
    pub edca: WmmAcParameters,
    /// The exchange that protects its PPDUs.
    pub protection: Protection,
    /// The IEEE encoding of the station's Minimum MPDU Start Spacing.
    pub spacing: u8,
    /// The most subframes one of its aggregates holds.
    pub max_subframes: u16,
    /// The station's Maximum A-MPDU Length, in octets.
    pub max_length: u32,
}

impl AirPeer {
    /// A best-effort access point with the standard's default contention
    /// at `rate`, which aggregates up to the HT limit of 64 subframes and
    /// 65 535 octets with no start spacing, unprotected.
    #[must_use]
    pub fn best_effort(
        frame: Box<dyn FnMut(SequenceNumber) -> Vec<u8>>,
        meta: ModelRxMeta,
        rate: PhyRate,
        offered: Offered,
        window: PeerWindow,
        tid: Tid,
    ) -> Self {
        Self {
            frame,
            meta,
            air_overhead_octets: FCS_OCTETS,
            rate,
            offered,
            window,
            tid,
            edca: WmmParameterSet::STANDARD_DEFAULT.access_category(WmmAccessCategory::BestEffort),
            protection: Protection::None,
            spacing: 0,
            max_subframes: 64,
            max_length: 65_535,
        }
    }
}

/// Another network's use of the channel: a burst of `busy_micros` at the
/// start of each period, or as soon as the transmission on the channel ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusyChannel {
    /// The time from one burst to the next.
    pub period_micros: NonZeroU32,
    /// The length of each burst.
    pub busy_micros: u32,
}

/// The limits of the model's receive path. Each is a separate input: they
/// bind at different places.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelRxLimits {
    /// The frames the ring holds before the caller pulls them; a frame that
    /// finds it full is lost before the recipient's scoreboard.
    pub ring: usize,
    /// The received units the caller may hold at once: `RX_HELD`.
    pub held: usize,
    /// The units the network may hold at once: its credit.
    pub lent: usize,
}

/// The whole air of one model.
pub struct AirConfig {
    /// The seed of every backoff.
    pub seed: u64,
    /// The channel's times.
    pub timing: AirTiming,
    /// The subframes one data arena holds, 1 to the port's `ARENA_FRAMES`.
    pub arena_frames: u8,
    /// The receive path's limits.
    pub rx_limits: ModelRxLimits,
    /// The other networks on the channel.
    pub busy: Option<BusyChannel>,
    /// The access point's traffic.
    pub peer: Option<AirPeer>,
}

/// Why [`ModelAir::set_air_time`](crate::ModelAir::set_air_time) refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirConfigRefused {
    /// The arena's subframes lie outside 1 to `ARENA_FRAMES`.
    ArenaFrames,
    /// A busy burst does not end within its period.
    BusyPeriod,
    /// The access point can send no subframe.
    PeerSubframes,
    /// The air already runs on a medium.
    AlreadyTimed,
}

/// One exchange the station asked the medium for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirTicket(u64);

/// How far a ticket's exchange came.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirProgress {
    /// It waits for the channel or is on it.
    Pending,
    /// It ended with its response.
    Answered,
    /// It ended in a collision: no response came.
    Collided,
}

/// Who held the channel over one span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirHolder {
    Station,
    AccessPoint,
    /// The station and the access point at once.
    Collision,
    /// Another network.
    Busy,
}

/// One holder of the channel, from its first to its last microsecond.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirSpan {
    pub holder: AirHolder,
    pub start_micros: u64,
    pub end_micros: u64,
}

/// What ended the access point's aggregates: the limit that bound each one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PeerBounds {
    /// It had no more frames to send.
    pub offered: u64,
    /// The Block Ack window.
    pub window: u64,
    /// Its subframe limit.
    pub subframes: u64,
    /// The station's Maximum A-MPDU Length.
    pub length: u64,
    /// The rate's PPDU time limit.
    pub ppdu_time: u64,
    /// The TXOP limit.
    pub txop: u64,
}

/// One direction's totals.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AirSideReport {
    /// Exchanges that went on the air.
    pub exchanges: u64,
    /// Their subframes, a single MPDU as one.
    pub subframes: u64,
    /// Exchanges that collided.
    pub collisions: u64,
    /// The channel time they held.
    pub air_micros: u64,
}

/// What the medium did since its last reset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AirReport {
    /// The station's transmissions.
    pub station: AirSideReport,
    /// The access point's transmissions.
    pub peer: AirSideReport,
    /// The access point's subframes that landed in the ring.
    pub landed: u64,
    /// Its subframes the ring had no slot for.
    pub ring_drops: u64,
    /// The channel time other networks held.
    pub busy_micros: u64,
    /// What bound the access point's aggregates.
    pub bounds: PeerBounds,
}

/// What the medium asks of the air it runs on.
pub(crate) trait AirWorld {
    /// The agreement applied for `tid`.
    fn agreed_window(&self, tid: Tid) -> Option<u16>;
    /// Put `frame` in the ring: `false` without a slot.
    fn land(&self, frame: Vec<u8>, meta: ModelRxMeta) -> bool;
}

/// A deterministic stream of backoffs: SplitMix64, a test sequence and no
/// random source.
#[derive(Clone, Copy)]
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    /// A value from 0 to `high`, both included.
    fn up_to(&mut self, high: u32) -> u32 {
        let span = u64::from(high).saturating_add(1);
        let value = (self.next() >> 32).saturating_mul(span) >> 32;
        u32::try_from(value).unwrap_or(high)
    }
}

/// One side's contention state.
#[derive(Clone, Copy)]
struct Contender {
    access: AirAccess,
    /// The current contention window.
    cw: u32,
    /// The backoff slots left.
    slots: u32,
    /// The channel went idle at this instant; `None` before any
    /// transmission.
    idle: Option<u64>,
    /// The backoff after its own transmission still runs: a frame that
    /// arrives meanwhile takes it, however few slots it drew.
    posted: bool,
}

impl Contender {
    fn new(access: AirAccess) -> Self {
        Self {
            access,
            cw: access.cw_min,
            slots: 0,
            idle: None,
            posted: false,
        }
    }

    /// The instant its slots count down from: the end of AIFS.
    fn from(&self) -> u64 {
        self.idle.map_or(0, |idle| {
            idle.saturating_add(u64::from(self.access.aifs_micros))
        })
    }

    /// Contend by `access` from now on.
    fn adopt(&mut self, access: AirAccess) {
        self.access = access;
        self.cw = self.cw.max(access.cw_min).min(access.cw_max);
    }

    /// The instant its slots run out, when nothing else takes the channel.
    fn expiry(&self, slot: u32) -> u64 {
        self.from()
            .saturating_add(u64::from(self.slots).saturating_mul(u64::from(slot)))
    }

    /// Another transmission started at `at`: the slots that passed are gone.
    fn freeze(&mut self, at: u64, slot: u32) {
        self.posted &= at < self.expiry(slot);
        let passed = at
            .saturating_sub(self.from())
            .checked_div(u64::from(slot))
            .unwrap_or(0);
        self.slots = self
            .slots
            .saturating_sub(u32::try_from(passed).unwrap_or(u32::MAX));
    }

    /// The channel went idle at `end`.
    fn resume(&mut self, end: u64) {
        self.idle = Some(end);
    }

    /// Its own transmission ended at `end`: the window follows the result
    /// and the next backoff starts at once.
    fn finished(&mut self, end: u64, collided: bool, rng: &mut Seeded) {
        self.cw = if collided {
            self.cw
                .saturating_mul(2)
                .saturating_add(1)
                .min(self.access.cw_max)
        } else {
            self.access.cw_min
        };
        self.slots = rng.up_to(self.cw);
        self.posted = true;
        self.resume(end);
    }

    /// A frame arrived at `at` while the channel is `busy`: a side with no
    /// backoff left transmits at once only after an idle AIFS.
    fn arrived(&mut self, at: u64, busy: bool, rng: &mut Seeded) {
        if self.slots == 0 && !self.posted && (busy || at < self.from()) {
            self.slots = rng.up_to(self.cw);
        }
    }
}

/// One request of the station.
struct Request {
    ticket: AirTicket,
    exchange: AirExchange,
    access: AirAccess,
    subframes: u64,
    since: u64,
}

/// The access point's state.
struct Peer {
    config: AirPeer,
    contender: Contender,
    started: u64,
    /// The new frames it took so far.
    taken: u64,
    next_sequence: SequenceNumber,
    /// Sent and not acknowledged, oldest first.
    unacked: VecDeque<SequenceNumber>,
    /// It contends for the frames it has since this instant.
    since: Option<u64>,
}

impl Peer {
    /// The frames that arrived up to `at`, taken or not.
    fn arrived(&self, at: u64) -> u64 {
        match self.config.offered {
            Offered::Saturated => u64::MAX,
            Offered::FramesPerSecond(rate) => {
                at.saturating_sub(self.started)
                    .saturating_mul(u64::from(rate.get()))
                    / 1_000_000
            }
        }
    }

    /// The next instant a frame arrives; `None` when one always waits.
    fn next_arrival(&self) -> Option<u64> {
        match self.config.offered {
            Offered::Saturated => None,
            Offered::FramesPerSecond(rate) => {
                let wanted = self.taken.saturating_add(1).saturating_mul(1_000_000);
                Some(self.started.saturating_add(ceil_div(wanted, rate.get())))
            }
        }
    }

    /// Whether it has a frame to send at `at`.
    fn has_work(&self, at: u64) -> bool {
        !self.unacked.is_empty() || self.arrived(at) > self.taken
    }
}

/// How far `later` lies after `earlier`, modulo the sequence space.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.1.8 (Sequence Number field: modulo 4096).
const fn forward_distance(earlier: SequenceNumber, later: SequenceNumber) -> u16 {
    later.get().wrapping_sub(earlier.get()) & 0x0fff
}

/// `value` over `divisor`, rounded up.
fn ceil_div(value: u64, divisor: u32) -> u64 {
    let divisor = u64::from(divisor);
    let whole = value.checked_div(divisor).unwrap_or(u64::MAX);
    let rest = value.checked_rem(divisor).unwrap_or(0);
    whole.saturating_add(u64::from(rest != 0))
}

/// The access point's PPDU on the channel.
struct PeerPpdu {
    /// Each frame's Sequence Number, image, landing instant and whether it
    /// landed, in air order.
    frames: Vec<PeerFrame>,
    /// The next frame to land.
    next: usize,
}

struct PeerFrame {
    sequence: SequenceNumber,
    image: Vec<u8>,
    at: u64,
    landed: bool,
}

/// The transmission on the channel.
struct OnAir {
    started: u64,
    end: u64,
    station: Option<AirTicket>,
    peer: Option<PeerPpdu>,
    collided: bool,
}

impl OnAir {
    fn holder(&self) -> AirHolder {
        match (self.station.is_some(), self.peer.is_some()) {
            (true, true) => AirHolder::Collision,
            (true, false) => AirHolder::Station,
            (false, true) => AirHolder::AccessPoint,
            (false, false) => AirHolder::Busy,
        }
    }

    /// The next instant the transmission does something.
    fn next_at(&self) -> u64 {
        self.peer
            .as_ref()
            .and_then(|ppdu| ppdu.frames.get(ppdu.next))
            .map_or(self.end, |frame| frame.at.min(self.end))
    }
}

/// The busy channel's next burst.
#[derive(Clone, Copy)]
struct Bursts {
    config: BusyChannel,
    next: u64,
}

/// One aggregate the access point built.
struct Built {
    frames: Vec<(SequenceNumber, Vec<u8>, u32)>,
    exchange: AirExchange,
}

/// The shared channel.
pub(crate) struct Medium {
    timing: AirTiming,
    rng: Seeded,
    station: Contender,
    requests: VecDeque<Request>,
    ended: Vec<(AirTicket, AirProgress)>,
    next_ticket: u64,
    peer: Option<Peer>,
    bursts: Option<Bursts>,
    on_air: Option<OnAir>,
    spans: Vec<AirSpan>,
    report: AirReport,
}

impl Medium {
    pub(crate) fn new(
        at: u64,
        seed: u64,
        timing: AirTiming,
        busy: Option<BusyChannel>,
        peer: Option<AirPeer>,
    ) -> Self {
        let mut medium = Self {
            timing,
            rng: Seeded(seed),
            station: Contender::new(AirAccess::of(
                timing,
                WmmParameterSet::STANDARD_DEFAULT.access_category(WmmAccessCategory::BestEffort),
            )),
            requests: VecDeque::new(),
            ended: Vec::new(),
            next_ticket: 0,
            peer: peer.map(|config| Peer {
                contender: Contender::new(AirAccess::of(timing, config.edca)),
                config,
                started: at,
                taken: 0,
                next_sequence: SequenceNumber::ZERO,
                unacked: VecDeque::new(),
                since: None,
            }),
            bursts: busy.map(|config| Bursts { config, next: at }),
            on_air: None,
            spans: Vec::new(),
            report: AirReport::default(),
        };
        let _arrived: bool = medium.refresh_peer(at);
        medium
    }

    /// Ask for an exchange of the station at `at`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one PPDU's fields, which the port holds apart"
    )]
    pub(crate) fn request(
        &mut self,
        at: u64,
        rate: PhyRate,
        psdu_octets: u32,
        protection: Protection,
        response: AirResponse,
        parameters: WmmAcParameters,
        subframes: u8,
    ) -> Result<AirTicket, AirTimeError> {
        let exchange = AirExchange::new(self.timing, rate, psdu_octets, protection, response)?;
        let access = AirAccess::of(self.timing, parameters);
        let subframes = u64::from(subframes);
        let ticket = AirTicket(self.next_ticket);
        self.next_ticket = self.next_ticket.saturating_add(1);
        if self.requests.is_empty() {
            self.station.adopt(access);
            let busy = self.on_air.is_some();
            self.station.arrived(at, busy, &mut self.rng);
        }
        self.requests.push_back(Request {
            ticket,
            exchange,
            access,
            subframes,
            since: at,
        });
        Ok(ticket)
    }

    /// How far the exchange of `ticket` came.
    pub(crate) fn progress(&self, ticket: AirTicket) -> AirProgress {
        self.ended
            .iter()
            .find(|(ended, _)| *ended == ticket)
            .map_or(AirProgress::Pending, |(_, progress)| *progress)
    }

    /// The consumer of `ticket` is done with it: a request still waiting
    /// leaves the queue. One on the channel keeps its time.
    pub(crate) fn forget(&mut self, ticket: AirTicket) {
        self.ended.retain(|(ended, _)| *ended != ticket);
        if self
            .on_air
            .as_ref()
            .is_none_or(|on_air| on_air.station != Some(ticket))
        {
            self.requests.retain(|request| request.ticket != ticket);
        }
    }

    /// What it did since the last reset.
    pub(crate) const fn report(&self) -> AirReport {
        self.report
    }

    /// Start counting anew.
    pub(crate) fn reset_report(&mut self) {
        self.report = AirReport::default();
        self.spans.clear();
    }

    /// Every transmission since the last reset, in order.
    pub(crate) fn spans(&self) -> &[AirSpan] {
        &self.spans
    }

    fn station_access(&self) -> Option<u64> {
        let request = self.requests.front()?;
        Some(
            self.station
                .expiry(self.timing.slot_micros)
                .max(request.since),
        )
    }

    fn peer_access(&self) -> Option<u64> {
        let peer = self.peer.as_ref()?;
        let since = peer.since?;
        Some(peer.contender.expiry(self.timing.slot_micros).max(since))
    }

    /// The next instant anything happens on the channel.
    pub(crate) fn next_event(&self) -> Option<u64> {
        if let Some(on_air) = &self.on_air {
            return Some(on_air.next_at());
        }
        let arrival = self
            .peer
            .as_ref()
            .filter(|peer| peer.since.is_none())
            .and_then(Peer::next_arrival);
        [
            self.station_access(),
            self.peer_access(),
            arrival,
            self.bursts.map(|bursts| bursts.next),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Serve every event up to `now`, each at its own instant, however late
    /// the clock's owner comes.
    pub(crate) fn advance(&mut self, now: u64, world: &impl AirWorld) -> Result<(), AirTimeError> {
        while let Some(at) = self.next_event() {
            if at > now {
                break;
            }
            if !self.fire(at, world)? {
                return Err(AirTimeError::Stalled);
            }
        }
        Ok(())
    }

    /// Serve the event due at `at`; whether anything happened, which an
    /// event always does unless the medium broke its own rules.
    fn fire(&mut self, at: u64, world: &impl AirWorld) -> Result<bool, AirTimeError> {
        if let Some(on_air) = self.on_air.take() {
            self.serve_on_air(on_air, world);
            return Ok(true);
        }
        let arrived = self.refresh_peer(at);
        if self.burst_due(at) {
            self.start_burst(at);
            return Ok(true);
        }
        let station = self.station_access() == Some(at);
        let peer = self.peer_access() == Some(at);
        if station || peer {
            self.start(at, station, peer, world)?;
        }
        Ok(arrived || self.on_air.is_some())
    }

    fn burst_due(&self, at: u64) -> bool {
        self.bursts.is_some_and(|bursts| bursts.next <= at)
    }

    /// Take the channel for a burst at `at`.
    fn start_burst(&mut self, at: u64) {
        let Some(bursts) = self.bursts.as_mut() else {
            return;
        };
        let end = at.saturating_add(u64::from(bursts.config.busy_micros));
        bursts.next = bursts
            .next
            .saturating_add(u64::from(bursts.config.period_micros.get()));
        self.freeze(at, true, true);
        self.report.busy_micros = self
            .report
            .busy_micros
            .saturating_add(end.saturating_sub(at));
        self.on_air = Some(OnAir {
            started: at,
            end,
            station: None,
            peer: None,
            collided: false,
        });
    }

    /// The sides that do not transmit at `at` lose the slots that passed.
    fn freeze(&mut self, at: u64, station: bool, peer: bool) {
        let slot = self.timing.slot_micros;
        if station {
            self.station.freeze(at, slot);
        }
        if peer && let Some(peer) = self.peer.as_mut() {
            peer.contender.freeze(at, slot);
        }
    }

    /// The sides whose access falls on `at` take the channel: both collide.
    fn start(
        &mut self,
        at: u64,
        station: bool,
        peer: bool,
        world: &impl AirWorld,
    ) -> Result<(), AirTimeError> {
        let built = if peer { self.build(at, world)? } else { None };
        let request = self.requests.front().filter(|_| station);
        let collided = request.is_some() && built.is_some();
        let held = |exchange: &AirExchange| {
            if collided {
                Ok(exchange.lost_micros)
            } else {
                exchange.occupancy_micros()
            }
        };
        let station_time = request
            .map(|request| Ok((request.ticket, request.subframes, held(&request.exchange)?)))
            .transpose()?;
        let peer_micros = built
            .as_ref()
            .map(|built| held(&built.exchange))
            .transpose()?;
        self.freeze(at, !station, !peer);
        let ppdu = built.map(|built| self.peer_ppdu(at, built, collided));
        let mut longest = 0;
        if let Some((_, subframes, micros)) = station_time {
            longest = longest.max(micros);
            let side = &mut self.report.station;
            side.exchanges = side.exchanges.saturating_add(1);
            side.subframes = side.subframes.saturating_add(subframes);
            side.air_micros = side.air_micros.saturating_add(u64::from(micros));
            side.collisions = side.collisions.saturating_add(u64::from(collided));
        }
        if let Some(micros) = peer_micros {
            longest = longest.max(micros);
            let side = &mut self.report.peer;
            side.air_micros = side.air_micros.saturating_add(u64::from(micros));
            side.collisions = side.collisions.saturating_add(u64::from(collided));
        }
        self.on_air = Some(OnAir {
            started: at,
            end: at.saturating_add(u64::from(longest)),
            station: station_time.map(|(ticket, _, _)| ticket),
            peer: ppdu,
            collided,
        });
        Ok(())
    }

    /// The landing instants of `built`, which starts at `at`.
    fn peer_ppdu(&mut self, at: u64, built: Built, collided: bool) -> PeerPpdu {
        let Built { frames, exchange } = built;
        let first = at.saturating_add(u64::from(exchange.protection_micros));
        let total: u64 = frames
            .iter()
            .fold(0_u64, |total, (_, _, octets)| {
                total.saturating_add(u64::from(*octets))
            })
            .max(1);
        let mut carried = 0_u64;
        let frames: Vec<PeerFrame> = frames
            .into_iter()
            .map(|(sequence, image, octets)| {
                carried = carried.saturating_add(u64::from(octets));
                PeerFrame {
                    sequence,
                    image,
                    at: first.saturating_add(
                        u64::from(exchange.ppdu_micros)
                            .saturating_mul(carried)
                            .checked_div(total)
                            .unwrap_or(0),
                    ),
                    landed: false,
                }
            })
            .collect();
        let side = &mut self.report.peer;
        side.exchanges = side.exchanges.saturating_add(1);
        side.subframes = side
            .subframes
            .saturating_add(u64::try_from(frames.len()).unwrap_or(u64::MAX));
        PeerPpdu {
            next: if collided { frames.len() } else { 0 },
            frames,
        }
    }

    /// The access point's aggregate at `at`, oldest frames first: its
    /// unacknowledged frames and then new ones in the window, as long as
    /// every limit holds. `None` with nothing to send.
    fn build(&mut self, at: u64, world: &impl AirWorld) -> Result<Option<Built>, AirTimeError> {
        let timing = self.timing;
        let Some(peer) = self.peer.as_mut() else {
            return Ok(None);
        };
        let window = match peer.config.window {
            PeerWindow::Agreed => world.agreed_window(peer.config.tid),
            PeerWindow::Assumed(window) => Some(window),
        };
        let rate = peer.config.rate;
        let aggregates = window.is_some_and(|window| window > 1)
            && rate
                .minimum_mpdu_spacing_bytes(peer.config.spacing)
                .is_some();
        let window = window.unwrap_or(1).max(1);
        let head = peer.unacked.front().copied().unwrap_or(peer.next_sequence);
        let spacing = if aggregates { peer.config.spacing } else { 0 };
        let mut run = if aggregates {
            Some(PsduRun::new(rate, spacing)?)
        } else {
            None
        };
        let txop =
            u32::from(peer.config.edca.txop_limit_units_32_us).saturating_mul(TXOP_UNIT_MICROS);
        let response = AirExchange::new(
            timing,
            rate,
            0,
            Protection::None,
            if aggregates {
                AirResponse::BlockAck
            } else {
                AirResponse::Ack
            },
        )?
        .response_micros;
        let retries: Vec<SequenceNumber> = peer.unacked.iter().copied().collect();
        let mut retries = retries.into_iter();
        let mut frames: Vec<(SequenceNumber, Vec<u8>, u32)> = Vec::new();
        // The window holds every frame from its head: the retries lie in it,
        // and no new frame goes past its end.
        let limit = if aggregates {
            usize::from(peer.config.max_subframes)
        } else {
            1
        };
        let bound = loop {
            if frames.len() >= limit {
                break if aggregates {
                    Bound::Subframes
                } else {
                    Bound::Window
                };
            }
            let retry = retries.next();
            let sequence = match retry {
                Some(sequence) => sequence,
                None if peer.arrived(at) <= peer.taken => break Bound::Offered,
                None if forward_distance(head, peer.next_sequence) >= window => {
                    break Bound::Window;
                }
                None => peer.next_sequence,
            };
            let image = (peer.config.frame)(sequence);
            let octets = u32::try_from(image.len())
                .ok()
                .and_then(|length| length.checked_add(peer.config.air_overhead_octets))
                .ok_or(AirTimeError::Unrepresentable)?;
            if let Some(run) = run.as_mut() {
                let next = run.after(octets)?;
                if next.length > peer.config.max_length {
                    break Bound::Length;
                }
                let ppdu = rate
                    .max_ppdu_duration_micros(next.length)
                    .ok_or(AirTimeError::Unrepresentable)?;
                if rate.max_txtime_micros().is_some_and(|limit| ppdu > limit) {
                    break Bound::PpduTime;
                }
                if txop != 0 && ppdu.saturating_add(response) > txop {
                    break Bound::Txop;
                }
                *run = next;
            }
            frames.push((sequence, image, octets));
            if retry.is_none() {
                peer.unacked.push_back(sequence);
                peer.next_sequence = SequenceNumber::from_low_bits(sequence.get().wrapping_add(1));
                peer.taken = peer.taken.saturating_add(1);
            }
        };
        if frames.is_empty() {
            return Err(AirTimeError::PeerFrameOutsideLimits);
        }
        let bounds = &mut self.report.bounds;
        let count = match bound {
            Bound::Offered => &mut bounds.offered,
            Bound::Window => &mut bounds.window,
            Bound::Subframes => &mut bounds.subframes,
            Bound::Length => &mut bounds.length,
            Bound::PpduTime => &mut bounds.ppdu_time,
            Bound::Txop => &mut bounds.txop,
        };
        *count = count.saturating_add(1);
        let psdu = match run.filter(|_| frames.len() > 1) {
            Some(run) => run.length,
            None => frames.first().map_or(0, |(_, _, octets)| *octets),
        };
        let response = if frames.len() > 1 {
            AirResponse::BlockAck
        } else {
            AirResponse::Ack
        };
        let exchange = AirExchange::new(timing, rate, psdu, peer.config.protection, response)?;
        Ok(Some(Built { frames, exchange }))
    }

    /// The transmission on the channel reached `at`: a frame lands, or it
    /// ends.
    fn serve_on_air(&mut self, mut on_air: OnAir, world: &impl AirWorld) {
        let meta = self.peer.as_ref().map(|peer| peer.config.meta);
        if let (Some(ppdu), Some(meta)) = (on_air.peer.as_mut(), meta)
            && let Some(frame) = ppdu.frames.get_mut(ppdu.next)
        {
            frame.landed = world.land(core::mem::take(&mut frame.image), meta);
            let counter = if frame.landed {
                &mut self.report.landed
            } else {
                &mut self.report.ring_drops
            };
            *counter = counter.saturating_add(1);
            ppdu.next = ppdu.next.saturating_add(1);
            self.on_air = Some(on_air);
            return;
        }
        self.finish(on_air);
    }

    /// The transmission ended: its sides draw again, the others resume, and
    /// a burst that waited for it takes the channel.
    fn finish(&mut self, on_air: OnAir) {
        let holder = on_air.holder();
        let OnAir {
            started,
            end,
            station,
            peer,
            collided,
        } = on_air;
        self.spans.push(AirSpan {
            holder,
            start_micros: started,
            end_micros: end,
        });
        if let Some(ticket) = station {
            self.requests.retain(|request| request.ticket != ticket);
            let progress = if collided {
                AirProgress::Collided
            } else {
                AirProgress::Answered
            };
            self.ended.push((ticket, progress));
            self.station.finished(end, collided, &mut self.rng);
        } else {
            self.station.resume(end);
        }
        if let Some(head) = self.requests.front_mut() {
            head.since = head.since.max(end);
            let access = head.access;
            self.station.adopt(access);
        }
        if let Some(state) = self.peer.as_mut() {
            match &peer {
                Some(ppdu) => {
                    for frame in ppdu.frames.iter().filter(|frame| frame.landed) {
                        state.unacked.retain(|sequence| *sequence != frame.sequence);
                    }
                    state.contender.finished(end, collided, &mut self.rng);
                    state.since = state.has_work(end).then_some(end);
                }
                None => state.contender.resume(end),
            }
        }
        let _arrived: bool = self.refresh_peer(end);
        if self.burst_due(end) {
            self.start_burst(end);
        }
    }

    /// The access point begins to contend once it has a frame.
    /// Whether it began to.
    fn refresh_peer(&mut self, at: u64) -> bool {
        let busy = self.on_air.is_some();
        if let Some(peer) = self.peer.as_mut()
            && peer.since.is_none()
            && peer.has_work(at)
        {
            peer.since = Some(at);
            peer.contender.arrived(at, busy, &mut self.rng);
            return true;
        }
        false
    }
}

/// The limit that ended one aggregate.
#[derive(Clone, Copy)]
enum Bound {
    Offered,
    Window,
    Subframes,
    Length,
    PpduTime,
    Txop,
}

#[cfg(test)]
mod tests;
