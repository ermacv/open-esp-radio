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
        0..2 => FRAME_CONTROL.get(offset).copied(),
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
    while EmbassyClock.now() < end
        && !(counts.peer_units >= UNITS
            && counts.descriptors / COUNT as u64 >= u64::from(LAPS)
            && counts.per_length.iter().all(|per| per.units >= PER_LENGTH))
    {
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
        let first = check(&joined, &mut counts, station, &mut peer, &lengths);
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
    let _ = writeln!(line, "LAPS spans {:?}", counts.spans);
    usb::text(console, line.as_bytes()).await;
    line.clear();
    let _ = writeln!(
        line,
        "LAPS end units={} peer_units={} descriptors={} laps={laps} formula_disagrees={} length_disagrees={} metadata_disagrees={} mismatch_units={} mismatch_bytes={} foreign_prefix={} unknown_length={} reserved_tail={}",
        counts.units,
        counts.peer_units,
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
    let ours = to == Some(station) && control == Some(FRAME_CONTROL);
    if !ours {
        if joined.steps.len() > 1 && holds_injector_body(joined.last()) {
            counts.foreign_prefix = counts.foreign_prefix.saturating_add(1);
        }
        return None;
    }
    let from = from?;
    let sender = *peer.get_or_insert(from);
    if from != sender {
        return None;
    }
    counts.peer_units = counts.peer_units.saturating_add(1);
    let mpdu = joined.len().saturating_sub(HEADER);
    // The tail's signal length is the PSDU's: the MPDU and its FCS.
    let tail = joined.array::<{ WifiRxControlTail::SIZE }>(WifiRxControlPrefix::SIZE);
    let prefix = joined.array::<{ WifiRxControlPrefix::SIZE }>(0);
    let signal =
        tail.map(|tail| usize::from(WifiRxControlTail::new(&tail).signal_length().length()));
    let rate = prefix.map(|prefix| WifiRxControlPrefix::new(&prefix).rate().rate());
    if signal != Some(mpdu + 4) {
        counts.length_disagrees = counts.length_disagrees.saturating_add(1);
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
