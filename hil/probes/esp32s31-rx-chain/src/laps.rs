//! Case D of `tasks/156`: frames longer than one buffer, joined from their
//! chains over many laps of a ring taken and recycled as the product does.
//!
//! The sender is the stand's injector as it is: non-QoS Data to the
//! station's address, its body LLC/SNAP `0x88b5` then each byte its offset
//! (`tools/lab/peers/esp32c5-wifi-injector`, `command_tx`), at lengths around
//! the ring's buffer. For every completed unit the probe joins its steps in
//! order (each step's bytes bounded by its own completed word: nothing past
//! the terminal LENGTH is read), checks the length by the formula
//! `(n − 1) × SIZE + last LENGTH`, and compares the frame with what the
//! injector sends: every byte but the duration and the sequence control,
//! which its driver writes.
use super::{BUFFER, COUNT, Storage, usb};
use core::fmt::Write as _;
use oer_chips_esp32s31_hal::{
    layouts::{WifiRxControlPrefix, WifiRxControlTail},
    radio::ieee80211::dma::rx_ring::RxRingLive,
    system::owner::RadioAccess,
};
use oer_chips_esp32s31_radio_ieee80211_backend::rx::receive::RELOAD_SETTLE_LIMIT;
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer, settle::Settle};

/// The RX-control prefix and its tail before the frame (no HE-SIG-B
/// options at the injector's 6 Mb/s).
const HEADER: usize = WifiRxControlPrefix::SIZE + WifiRxControlTail::SIZE;
/// The longest the run lasts.
const RUN_MS: u32 = 150_000;
/// The units of the injector's the run aims at, and the laps.
const UNITS: u32 = 10_000;
const LAPS: u32 = 100;
/// The units of each length the run aims at besides: every length is
/// covered, not only the first ones the host sends.
const PER_LENGTH: u32 = 1_000;
/// The most descriptors a unit is counted under; longer ones share the last.
const SPANS: usize = 16;
/// The mismatching units printed in full.
const SHOWN: usize = 16;
/// The injector's frame: data, no flags; its body's LLC/SNAP.
const FRAME_CONTROL: [u8; 2] = [0x08, 0x00];
/// The Retry flag of Frame Control's second octet (IEEE 802.11-2020
/// 9.2.4.1.6).
const RETRY: u8 = 0x08;
const SNAP: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0xb5];

/// The lengths the run's host sends, by the bytes a unit holds before its
/// FCS (`HEADER` + the MPDU): one less than a buffer, a buffer, one more,
/// two buffers, one more, a middle length, the injector's longest MPDU.
pub(super) const fn mpdu_lengths() -> [usize; 7] {
    [
        BUFFER - 1 - HEADER,
        BUFFER - HEADER,
        BUFFER + 1 - HEADER,
        2 * BUFFER - HEADER,
        2 * BUFFER + 1 - HEADER,
        700,
        1_500,
    ]
}

/// One length's counts.
#[derive(Clone, Copy, Default)]
struct PerLength {
    units: u32,
    spans_min: u32,
    spans_max: u32,
}

/// What the run counted.
#[derive(Default)]
struct Counts {
    units: u32,
    peer_units: u32,
    descriptors: u64,
    spans: [u32; SPANS],
    offset_set: u32,
    sosf_set: u32,
    formula_disagrees: u32,
    length_disagrees: u32,
    metadata_disagrees: u32,
    byte_mismatch_units: u32,
    byte_mismatches: u32,
    foreign_prefix: u32,
    unknown_length: u32,
    reserved_tail: u32,
    /// The injector's retransmissions among its units (Retry set).
    retries: u32,
    /// The injector's units by the padding after the frame up to a word.
    padding: [u32; 4],
    /// Units not the injector's whose last step holds the injector's body:
    /// their head index, span, and the first bytes of their Address 1.
    foreign: [[u32; 4]; 8],
    foreign_used: usize,
    per_length: [PerLength; 7],
    shown: [[u32; 5]; SHOWN],
    shown_used: usize,
}

/// One unit's steps, joined in order.
struct Joined<'a> {
    steps: heapless::Vec<&'a [u8], SPANS>,
}

impl Joined<'_> {
    fn len(&self) -> usize {
        self.steps.iter().map(|bytes| bytes.len()).sum()
    }

    fn get(&self, mut offset: usize) -> Option<u8> {
        for bytes in &self.steps {
            match bytes.get(offset) {
                Some(byte) => return Some(*byte),
                None => offset -= bytes.len(),
            }
        }
        None
    }

    fn array<const N: usize>(&self, offset: usize) -> Option<[u8; N]> {
        let mut out = [0_u8; N];
        for (at, cell) in out.iter_mut().enumerate() {
            *cell = self.get(offset.checked_add(at)?)?;
        }
        Some(out)
    }

    /// The bytes of the last step: what a foreign prefix would be joined to.
    fn last(&self) -> &[u8] {
        self.steps.last().copied().unwrap_or(&[])
    }
}

/// The injector's frame byte at `offset` of a frame from
/// `peer` to `station`, or `None` where its driver writes the byte.
fn expected(station: [u8; 6], peer: [u8; 6], offset: usize) -> Option<u8> {
    match offset {
        0 => FRAME_CONTROL.first().copied(),
        // The flags octet: compared with the Retry bit masked, above.
        1 => None,
        2..4 | 22..24 => None,
        4..10 => station.get(offset - 4).copied(),
        10..16 => peer.get(offset - 10).copied(),
        16..22 => peer.get(offset - 16).copied(),
        24..32 => SNAP.get(offset - 24).copied(),
        _ => Some(offset as u8),
    }
}

/// Whether `bytes` hold a run of 16 consecutive offsets of the injector's
/// body: the end of one of its frames.
fn holds_injector_body(bytes: &[u8]) -> bool {
    bytes.windows(16).any(|window| {
        window
            .windows(2)
            .all(|pair| matches!(pair, [a, b] if b.wrapping_sub(*a) == 1))
    })
}

/// Take and recycle the ring until the injector's units and the laps are
/// reached or [`RUN_MS`] passes; then print what was counted.
pub(super) async fn run(
    console: &mut usb::Console,
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    station: [u8; 6],
) -> Result<(), oer_portable_diagnostics::Name<32>> {
    let mut line = heapless::String::<512>::new();
    let lengths = mpdu_lengths();
    let _ = writeln!(
        line,
        "LAPS begin v=1 size={BUFFER} count={COUNT} lengths={lengths:?}"
    );
    usb::text(console, line.as_bytes()).await;
    let before = access.receive_statistics_snapshot().primary;
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(RUN_MS))
        .ok_or(oer_portable_diagnostics::name!("laps-deadline"; 32))?;
    let mut counts = Counts::default();
    let mut peer: Option<[u8; 6]> = None;
    #[cfg(feature = "amsdu")]
    let mut cases = c::Cases::default();
    #[cfg(feature = "amsdu")]
    let mut last_peer = None;
    while EmbassyClock.now() < end && !done(&counts) {
        // Case C ends when its sender has been quiet for a while.
        #[cfg(feature = "amsdu")]
        if let Some(at) = last_peer
            && EmbassyClock.now() >= at
        {
            break;
        }
        storage
            .observe_reserved(ring, access.rx_dma())
            .map_err(|_| oer_portable_diagnostics::name!("laps-reserved"; 32))?;
        ring.complete_pending_reload(
            access.rx_dma(),
            Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
        )
        .map_err(|_| oer_portable_diagnostics::name!("laps-reload"; 32))?;
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| oer_portable_diagnostics::name!("laps-cursor"; 32))?;
        let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| oer_portable_diagnostics::name!("laps-take"; 32))?
        else {
            if let Some(at) = EmbassyClock.now().checked_add(Duration::from_micros(50)) {
                EmbassyClock.wait_until(at).await;
            }
            continue;
        };
        let count = unit.descriptor_count();
        counts.units = counts.units.saturating_add(1);
        counts.descriptors = counts.descriptors.saturating_add(count as u64);
        if let Some(slot) = counts.spans.get_mut(count.min(SPANS) - 1) {
            *slot = slot.saturating_add(1);
        }
        if unit.metadata().continues_reserved_landing() {
            counts.reserved_tail = counts.reserved_tail.saturating_add(1);
        }
        // The steps, their words and the formula's length.
        let mut joined = Joined {
            steps: heapless::Vec::new(),
        };
        let mut formula = 0_usize;
        for step in 0..count {
            let step = unit
                .step(step)
                .map_err(|_| oer_portable_diagnostics::name!("laps-step"; 32))?;
            let word = step.descriptor_word0();
            counts.offset_set = counts.offset_set.saturating_add(u32::from(word.offset()));
            counts.sosf_set = counts.sosf_set.saturating_add(u32::from(word.sosf()));
            formula +=
                if step.descriptor_index() == unit.head_index().wrapping_add(count - 1) % COUNT {
                    usize::from(word.length())
                } else {
                    usize::from(word.size())
                };
            let _ = joined.steps.push(step.bytes());
        }
        if formula != unit.total_length() || formula != joined.len() {
            counts.formula_disagrees = counts.formula_disagrees.saturating_add(1);
        }
        #[cfg(not(feature = "amsdu"))]
        let first = check(&joined, &mut counts, station, &mut peer, &lengths);
        #[cfg(feature = "amsdu")]
        let first = {
            let before = counts.peer_units;
            let first = c::check(&joined, &mut counts, &mut cases, station, &mut peer);
            // Cell E: the pairwise key under the sender's address, once it is
            // known from its first (unprotected) frame.
            #[cfg(feature = "ccmp")]
            if cases.key.is_none()
                && let Some(sender) = peer
            {
                let mut hal = access.wifi_mac_hal();
                hal.configure_ccmp_association(
                    oer_chips_esp32s31_hal::types::MacInterface::Station,
                    oer_chips_esp32s31_hal::types::MacSppMode::Unauthenticated,
                );
                cases.key = Some(hal.install_station_ccmp_entry(
                    c::KEY_ENTRY,
                    oer_chips_esp32s31_hal::types::MacCcmpKeyIdentity::Pairwise { peer: sender },
                    &c::case_c::TEMPORAL_KEY,
                ));
            }
            if counts.peer_units != before {
                last_peer = EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(c::QUIET_MS));
            }
            first
        };
        drop(joined);
        if let Some(first) = first
            && let Some(slot) = counts.shown.get_mut(counts.shown_used)
        {
            // The unit's head, span, length and its first mismatching offset,
            // with the head descriptor's word.
            *slot = [
                unit.head_index() as u32,
                count as u32,
                unit.total_length() as u32,
                first as u32,
                unit.metadata().descriptor_address(),
            ];
            counts.shown_used += 1;
        }
        let credit = unit
            .retain_discard()
            .ok_or(oer_portable_diagnostics::name!("laps-retain"; 32))?;
        let _appended = credit
            .append(access.rx_dma())
            .map_err(|_| oer_portable_diagnostics::name!("laps-append"; 32))?;
        ring.complete_pending_reload(
            access.rx_dma(),
            Settle::new(RELOAD_SETTLE_LIMIT, &EmbassyClock),
        )
        .map_err(|_| oer_portable_diagnostics::name!("laps-reload"; 32))?;
    }
    let after = access.receive_statistics_snapshot().primary;
    let laps = counts.descriptors / COUNT as u64;
    #[cfg(feature = "amsdu")]
    c::print(console, &mut line, &cases).await;
    for (length, per) in lengths.iter().zip(&counts.per_length) {
        line.clear();
        let _ = writeln!(
            line,
            "LAPS length mpdu={length} unit={} units={} spans={}..{}",
            length + HEADER,
            per.units,
            per.spans_min,
            per.spans_max
        );
        usb::text(console, line.as_bytes()).await;
    }
    line.clear();
    let _ = writeln!(
        line,
        "LAPS spans {:?} padding {:?}",
        counts.spans, counts.padding
    );
    usb::text(console, line.as_bytes()).await;
    line.clear();
    let _ = writeln!(
        line,
        "LAPS end units={} peer_units={} retries={} descriptors={} laps={laps} formula_disagrees={} length_disagrees={} metadata_disagrees={} mismatch_units={} mismatch_bytes={} foreign_prefix={} unknown_length={} reserved_tail={}",
        counts.units,
        counts.peer_units,
        counts.retries,
        counts.descriptors,
        counts.formula_disagrees,
        counts.length_disagrees,
        counts.metadata_disagrees,
        counts.byte_mismatch_units,
        counts.byte_mismatches,
        counts.foreign_prefix,
        counts.unknown_length,
        counts.reserved_tail,
    );
    usb::text(console, line.as_bytes()).await;
    line.clear();
    let _ = writeln!(
        line,
        "LAPS bits offset_set={} sosf_set={} mac mpdu={} fcs_error={} buffer_full={} data_success={}",
        counts.offset_set,
        counts.sosf_set,
        after.mpdu_count.wrapping_sub(before.mpdu_count),
        after.fcs_error.wrapping_sub(before.fcs_error),
        after.buffer_full.wrapping_sub(before.buffer_full),
        after.data_success.wrapping_sub(before.data_success),
    );
    usb::text(console, line.as_bytes()).await;
    for foreign in counts.foreign.iter().take(counts.foreign_used) {
        line.clear();
        let _ = writeln!(
            line,
            "LAPS foreign spans={} control_duration={:#010x} address1={:#010x} address2={:#010x}",
            foreign[0], foreign[1], foreign[2], foreign[3]
        );
        usb::text(console, line.as_bytes()).await;
    }
    for shown in counts.shown.iter().take(counts.shown_used) {
        line.clear();
        let _ = writeln!(
            line,
            "LAPS mismatch head={} spans={} length={} first={} descriptor={:#x}",
            shown[0], shown[1], shown[2], shown[3], shown[4]
        );
        usb::text(console, line.as_bytes()).await;
    }
    Ok(())
}

/// Whether case D's run has what it aims at.
#[cfg(not(feature = "amsdu"))]
fn done(counts: &Counts) -> bool {
    counts.peer_units >= UNITS
        && counts.descriptors / COUNT as u64 >= u64::from(LAPS)
        && counts.per_length.iter().all(|per| per.units >= PER_LENGTH)
}

/// Case C's run ends at its deadline or when its sender is quiet.
#[cfg(feature = "amsdu")]
const fn done(_counts: &Counts) -> bool {
    false
}

/// Case C's receive (`tasks/154`, `159`): the sender's frames checked
/// byte by byte against [`case_c`]'s layout, counted per case and round.
#[cfg(feature = "amsdu")]
mod c {
    pub(super) use super::super::case_c::{self, CASES, Case, ROUNDS};
    /// Cell E's pairwise key entry: the station's, as the backend's
    /// (`STA_PAIRWISE_HARDWARE_INDEX`).
    #[cfg(feature = "ccmp")]
    pub(super) const KEY_ENTRY: u8 = 4;
    use super::*;

    /// The rounds the sender sends: case C's, or cell E's.
    #[cfg(not(feature = "ccmp"))]
    const RECEIVED_ROUNDS: u32 = ROUNDS;
    #[cfg(feature = "ccmp")]
    const RECEIVED_ROUNDS: u32 = case_c::CELL_ROUNDS;

    /// The sender's quiet that ends the run once it has begun.
    pub(super) const QUIET_MS: u32 = 5_000;
    const WORDS: usize = (ROUNDS as usize).div_ceil(32);
    /// The missing rounds printed per case.
    const MISSING: usize = 8;

    /// One case's counts.
    #[derive(Clone, Copy)]
    pub(super) struct PerCase {
        units: u32,
        /// Units of a round not seen before.
        distinct: u32,
        retries: u32,
        length_disagrees: u32,
        mismatch_units: u32,
        mismatch_bytes: u32,
        /// Units whose counters name more than one frame: a foreign prefix
        /// by content.
        counter_disagrees: u32,
        spans_min: u32,
        spans_max: u32,
        first_mismatch: Option<(u32, u32)>,
        /// The first MPDU length that disagreed with the case's.
        length_seen: Option<u32>,
        seen: [u32; WORDS],
    }

    impl Default for PerCase {
        fn default() -> Self {
            Self {
                units: 0,
                distinct: 0,
                retries: 0,
                length_disagrees: 0,
                mismatch_units: 0,
                mismatch_bytes: 0,
                counter_disagrees: 0,
                spans_min: 0,
                spans_max: 0,
                first_mismatch: None,
                length_seen: None,
                seen: [0; WORDS],
            }
        }
    }

    /// Every case's counts, and the sender's units no case names.
    #[derive(Default)]
    pub(super) struct Cases {
        per: [PerCase; CASES.len()],
        unknown: u32,
        /// Cell E: how the pairwise key's installation ended.
        #[cfg(feature = "ccmp")]
        pub(super) key: Option<oer_chips_esp32s31_hal::types::MacKeyInstallOutcome>,
    }

    /// Check one unit to the station: the sender's when its Frame Control
    /// (Retry masked) is one of the cases' and it comes from the sender
    /// learned first.
    pub(super) fn check(
        joined: &Joined<'_>,
        counts: &mut Counts,
        cases: &mut Cases,
        station: [u8; 6],
        peer: &mut Option<[u8; 6]>,
    ) -> Option<usize> {
        let to = joined.array::<6>(HEADER + 4);
        let from = joined.array::<6>(HEADER + 10)?;
        let [kind, flags] = joined.array::<2>(HEADER)?;
        // Cell E (`ccmp`): the QoS cases come protected, the hello frames not.
        let protected = cfg!(feature = "ccmp") && kind == 0x88;
        let flags_expected = if protected { case_c::PROTECTED } else { 0 };
        if to != Some(station) || flags & !RETRY != flags_expected || !matches!(kind, 0x88 | 0x08) {
            return None;
        }
        let sender = *peer.get_or_insert(from);
        if from != sender {
            return None;
        }
        counts.peer_units = counts.peer_units.saturating_add(1);
        let probe = if kind == 0x88 {
            Case::Amsdu2
        } else {
            Case::Provenance700
        };
        let counter_at = if protected {
            probe.protected_counter_at()
        } else {
            probe.counter_at()
        };
        let counter = joined
            .array::<4>(HEADER + counter_at)
            .map(u32::from_le_bytes);
        let Some((case, counter)) =
            counter.and_then(|counter| Some((Case::from_counter(counter)?, counter)))
        else {
            cases.unknown = cases.unknown.saturating_add(1);
            return None;
        };
        let at = CASES.iter().position(|each| *each == case)?;
        let per = cases.per.get_mut(at)?;
        per.units = per.units.saturating_add(1);
        if flags & RETRY != 0 {
            per.retries = per.retries.saturating_add(1);
        }
        let spans = joined.steps.len() as u32;
        per.spans_min = if per.units == 1 {
            spans
        } else {
            per.spans_min.min(spans)
        };
        per.spans_max = per.spans_max.max(spans);
        let round = Case::round(counter);
        if let Some(word) = per.seen.get_mut((round / 32) as usize) {
            let bit = 1 << (round % 32);
            if *word & bit == 0 {
                *word |= bit;
                per.distinct = per.distinct.saturating_add(1);
            }
        }
        // The frame's length from the RX-control tail, as in case D.
        let tail = joined.array::<{ WifiRxControlTail::SIZE }>(WifiRxControlPrefix::SIZE);
        let signal =
            tail.map(|tail| usize::from(WifiRxControlTail::new(&tail).signal_length().length()));
        let held = joined.len().saturating_sub(HEADER);
        let mpdu = signal.and_then(|signal| signal.checked_sub(4));
        let length = if protected {
            case.protected_length()
        } else {
            case.length()
        };
        match mpdu.and_then(|mpdu| Some((mpdu, held.checked_sub(mpdu)?))) {
            Some((mpdu, padding @ 0..4)) if mpdu == length => {
                if let Some(slot) = counts.padding.get_mut(padding) {
                    *slot = slot.saturating_add(1);
                }
            }
            _ => {
                per.length_disagrees = per.length_disagrees.saturating_add(1);
                // The tail's MPDU length and the bytes the unit holds after
                // the header.
                per.length_seen
                    .get_or_insert(((mpdu.unwrap_or(0) as u32) << 16) | held as u32);
                // Cell E compares what the unit holds: its MIC may be cut.
                if !protected {
                    return None;
                }
            }
        }
        let mut mismatches = 0_u32;
        let mut foreign = false;
        let mut first = None;
        for offset in 0..length.min(held) {
            let want = if protected {
                // The packet number and the MIC are not compared.
                match case_c::protected_expected(case, sender, station, counter, offset) {
                    Some(want) => want,
                    None => continue,
                }
            } else {
                case_c::byte(case, sender, station, counter, offset)
            };
            let got = joined.get(HEADER + offset);
            let same = match offset {
                // The Retry flag, the Duration and the Sequence Control are
                // the MAC's.
                1 => got.map(|got| got & !RETRY) == Some(want),
                2..4 | 22..24 => true,
                _ => got == Some(want),
            };
            if !same {
                mismatches = mismatches.saturating_add(1);
                first.get_or_insert(offset);
                foreign |= case.counter_place(if protected {
                    offset.saturating_sub(case_c::CCMP_HEADER)
                } else {
                    offset
                });
            }
        }
        if mismatches != 0 {
            per.mismatch_units = per.mismatch_units.saturating_add(1);
            per.mismatch_bytes = per.mismatch_bytes.saturating_add(mismatches);
            per.first_mismatch
                .get_or_insert((counter, first.unwrap_or(0) as u32));
            counts.byte_mismatch_units = counts.byte_mismatch_units.saturating_add(1);
        }
        if foreign {
            per.counter_disagrees = per.counter_disagrees.saturating_add(1);
        }
        first
    }

    /// One `CASE` line a case: its units, the rounds seen, what disagreed,
    /// and the first rounds never completed.
    pub(super) async fn print(
        console: &mut usb::Console,
        line: &mut heapless::String<512>,
        cases: &Cases,
    ) {
        for (case, per) in CASES.iter().zip(&cases.per) {
            // Cell E sends the QoS cases and, before them, unprotected hello
            // frames of case 5's layout; it sends no round of cases 5 and 6,
            // so they have no absent rounds to count (`tasks/172`).
            if cfg!(feature = "ccmp") && !case.qos() {
                line.clear();
                let _ = writeln!(
                    line,
                    "CASE tag={} hello units={} length_disagrees={} mismatch_units={}",
                    case.tag(),
                    per.units,
                    per.length_disagrees,
                    per.mismatch_units,
                );
                usb::text(console, line.as_bytes()).await;
                continue;
            }
            let mut missing = heapless::Vec::<u32, MISSING>::new();
            let mut absent = 0_u32;
            for round in 0..RECEIVED_ROUNDS {
                let seen = per
                    .seen
                    .get((round / 32) as usize)
                    .is_some_and(|word| word & 1 << (round % 32) != 0);
                if !seen {
                    absent = absent.saturating_add(1);
                    let _ = missing.push(round);
                }
            }
            line.clear();
            let _ = writeln!(
                line,
                "CASE tag={} mpdu={} units={} distinct={} retries={} absent={absent} length_disagrees={} mismatch_units={} mismatch_bytes={} counter_disagrees={} spans={}..{} first_mismatch={:?} length_seen={:x?} absent_first={:?}",
                case.tag(),
                case.length(),
                per.units,
                per.distinct,
                per.retries,
                per.length_disagrees,
                per.mismatch_units,
                per.mismatch_bytes,
                per.counter_disagrees,
                per.spans_min,
                per.spans_max,
                per.first_mismatch,
                per.length_seen,
                missing.as_slice(),
            );
            usb::text(console, line.as_bytes()).await;
        }
        // By the frame before (`tasks/160`): completed / sent, the last
        // column the run's first frame. Cell E sends the QoS cases alone,
        // so case C's order does not name its frames' neighbours.
        #[cfg(not(feature = "ccmp"))]
        for (case, per) in CASES.iter().zip(&cases.per) {
            let mut after = [[0_u32; 2]; CASES.len() + 1];
            for round in 0..ROUNDS {
                let seen = per
                    .seen
                    .get((round / 32) as usize)
                    .is_some_and(|word| word & 1 << (round % 32) != 0);
                if let Some(cell) = after.get_mut(case_c::before_index(round, *case)) {
                    cell[0] = cell[0].saturating_add(u32::from(seen));
                    cell[1] = cell[1].saturating_add(1);
                }
            }
            line.clear();
            let _ = writeln!(line, "PLACE tag={} after={:?}", case.tag(), after);
            usb::text(console, line.as_bytes()).await;
        }
        // The rounds completed, for the air capture's comparison.
        for (case, per) in CASES.iter().zip(&cases.per) {
            for (at, words) in per.seen.chunks(16).enumerate() {
                line.clear();
                let _ = write!(line, "SEEN tag={} from={}", case.tag(), at * 16 * 32);
                for word in words {
                    let _ = write!(line, " {word:08x}");
                }
                let _ = writeln!(line);
                usb::text(console, line.as_bytes()).await;
            }
        }
        line.clear();
        #[cfg(feature = "ccmp")]
        let _ = writeln!(line, "CASE unknown={} key={:?}", cases.unknown, cases.key);
        #[cfg(not(feature = "ccmp"))]
        let _ = writeln!(line, "CASE unknown={}", cases.unknown);
        usb::text(console, line.as_bytes()).await;
    }
}

/// Classify one joined unit: the injector's, checked byte by byte and by its
/// metadata; another's, checked for the injector's body at its end (a
/// foreign prefix joined to the injector's terminal). The first offset of
/// the frame that differs, for an injector's unit with a mismatch.
fn check(
    joined: &Joined<'_>,
    counts: &mut Counts,
    station: [u8; 6],
    peer: &mut Option<[u8; 6]>,
    lengths: &[usize; 7],
) -> Option<usize> {
    let to = joined.array::<6>(HEADER + 4);
    let from = joined.array::<6>(HEADER + 10);
    let control = joined.array::<2>(HEADER);
    // The Retry bit (Frame Control flags bit 3) set by a retransmission is
    // masked: a retry is compared like its first attempt.
    let ours = to == Some(station)
        && control.map(|[kind, flags]| [kind, flags & !RETRY]) == Some(FRAME_CONTROL);
    if ours && control.is_some_and(|[_, flags]| flags & RETRY != 0) {
        counts.retries = counts.retries.saturating_add(1);
    }
    if !ours {
        if joined.steps.len() > 1 && holds_injector_body(joined.last()) {
            counts.foreign_prefix = counts.foreign_prefix.saturating_add(1);
            if let Some(slot) = counts.foreign.get_mut(counts.foreign_used) {
                let head = joined.array::<4>(HEADER).map_or(0, u32::from_be_bytes);
                let address = joined.array::<4>(HEADER + 4).map_or(0, u32::from_be_bytes);
                let sender = joined.array::<4>(HEADER + 10).map_or(0, u32::from_be_bytes);
                *slot = [joined.steps.len() as u32, head, address, sender];
                counts.foreign_used += 1;
            }
        }
        return None;
    }
    let from = from?;
    let sender = *peer.get_or_insert(from);
    if from != sender {
        return None;
    }
    counts.peer_units = counts.peer_units.saturating_add(1);
    // The unit's bytes past the header, by its descriptors' lengths.
    let held = joined.len().saturating_sub(HEADER);
    // The frame's own length: the tail's signal length is the PSDU's, the
    // MPDU and its FCS. The descriptors' lengths round the unit up to a
    // word (run d2): the bytes between are padding, never compared.
    let tail = joined.array::<{ WifiRxControlTail::SIZE }>(WifiRxControlPrefix::SIZE);
    let prefix = joined.array::<{ WifiRxControlPrefix::SIZE }>(0);
    let signal =
        tail.map(|tail| usize::from(WifiRxControlTail::new(&tail).signal_length().length()));
    let rate = prefix.map(|prefix| WifiRxControlPrefix::new(&prefix).rate().rate());
    let Some(mpdu) = signal.and_then(|signal| signal.checked_sub(4)) else {
        counts.length_disagrees = counts.length_disagrees.saturating_add(1);
        return None;
    };
    // The unit holds the frame and at most a word's padding after it.
    match held.checked_sub(mpdu) {
        Some(padding @ 0..4) if (HEADER + mpdu).next_multiple_of(4) == HEADER + held => {
            if let Some(slot) = counts.padding.get_mut(padding) {
                *slot = slot.saturating_add(1);
            }
        }
        _ => counts.length_disagrees = counts.length_disagrees.saturating_add(1),
    }
    // The injector's 6 Mb/s (`MODE … 6m`): the PLCP rate code 0xb.
    if rate != Some(0xb) {
        counts.metadata_disagrees = counts.metadata_disagrees.saturating_add(1);
    }
    match lengths.iter().position(|length| *length == mpdu) {
        Some(at) => {
            if let Some(per) = counts.per_length.get_mut(at) {
                let spans = joined.steps.len() as u32;
                per.spans_min = if per.units == 0 {
                    spans
                } else {
                    per.spans_min.min(spans)
                };
                per.spans_max = per.spans_max.max(spans);
                per.units = per.units.saturating_add(1);
            }
        }
        None => counts.unknown_length = counts.unknown_length.saturating_add(1),
    }
    let mut mismatches = 0_u32;
    let mut first = None;
    for offset in 0..mpdu {
        if let Some(want) = expected(station, sender, offset)
            && joined.get(HEADER + offset) != Some(want)
        {
            mismatches = mismatches.saturating_add(1);
            first.get_or_insert(offset);
        }
    }
    if mismatches != 0 {
        counts.byte_mismatch_units = counts.byte_mismatch_units.saturating_add(1);
        counts.byte_mismatches = counts.byte_mismatches.saturating_add(mismatches);
    }
    first
}
