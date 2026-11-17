//! Part two's receiver (`tasks/158`, A2): the same bring-up and receive
//! start, then only receiving, so that the board acknowledges the frames
//! the transmitting probe sends to its station address. It counts them by
//! the counter each carries (`transmit::counter`): the second runs' frames
//! after the pulses by their chain, the others by their class, with their
//! lengths.
use super::*;

/// How long the receiver listens.
const LISTEN_MS: u32 = 420_000;
/// The receive windows it listens in.
const WINDOW_MS: u32 = 100;
/// The counter's place in the frame: after the action's category and OUI.
const COUNTER: usize = 28;
/// The counter classes (`transmit::counter`).
const SECOND: u32 = 0x8000_0000;
const OBSERVED: u32 = 0x4000_0000;
const PULSE: u32 = 0x2000_0000;
const CLASSES: u32 = 0xe000_0000;
/// The action frame the transmitting probe sends.
const ACTION: u8 = 0xd0;
const CATEGORY: u8 = 0x7f;

/// What the receiver counted.
#[derive(Default)]
struct Counted {
    units: u32,
    to_station: u32,
    observed: u32,
    stops: u32,
    pulses: u32,
    second: u32,
    second_seen: [u32; 4],
    short: u32,
    long: u32,
    other_length: u32,
    sender: [u8; 6],
}

pub(super) struct ReceiverParts<'a> {
    pub(super) storage: &'static Storage,
    pub(super) station: [u8; 6],
    pub(super) journal: &'a mut Journal,
}

/// Listen for [`LISTEN_MS`], taking and recycling every unit and counting
/// the transmitting probe's frames to the station.
pub(super) async fn series(
    parts: ReceiverParts<'_>,
    mut access: RadioAccess,
    clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mut ring: Ring,
) -> Result<(), Failure> {
    let ReceiverParts {
        storage,
        station,
        journal,
    } = parts;
    mark(80);
    let counted_before = statistics(&access);
    let mut counted = Counted::default();
    for _ in 0..LISTEN_MS / WINDOW_MS {
        listen(storage, &mut ring, &mut access, station, &mut counted).await?;
    }
    let counted_after = statistics(&access);
    let sender = counted.sender;
    journal.record(
        Kind::Rx,
        &[
            counted.units,
            counted.to_station,
            counted.observed,
            counted.stops,
            counted.pulses,
            counted.second,
            counted
                .second_seen
                .iter()
                .map(|word| word.count_ones())
                .sum(),
            counted.second_seen[0],
            counted.second_seen[1],
            counted.second_seen[2],
            counted.second_seen[3],
            counted.short,
            counted.long,
            counted.other_length,
            u32::from_be_bytes([0, 0, sender[0], sender[1]]),
            u32::from_be_bytes([sender[2], sender[3], sender[4], sender[5]]),
            advance(counted_before, counted_after, 0, 1),
            advance(counted_before, counted_after, 2, 3),
        ],
    );
    mark(81);
    // Every owner stays held until the leased host reset.
    let _held = (access, clocks_on, ring);
    Ok(())
}

/// One window's units, each taken, counted and recycled.
async fn listen(
    storage: &Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    station: [u8; 6],
    counted: &mut Counted,
) -> Result<(), Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(WINDOW_MS))
        .ok_or((phase::RECEIVE, 0x30))?;
    while EmbassyClock.now() < end {
        storage
            .observe_reserved(ring, access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x31))?;
        ring.complete_pending_reload(access.rx_dma(), Settle::new(RELOAD_SETTLE, &EmbassyClock))
            .map_err(|_| (phase::RECEIVE, 0x32))?;
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x33))?;
        let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| (phase::RECEIVE, 0x34))?
        else {
            wait_micros(50).await;
            continue;
        };
        counted.units = counted.units.saturating_add(1);
        if let Ok(step) = unit.step(0) {
            count(step.bytes(), station, counted);
        }
        let credit = unit.retain_discard().ok_or((phase::RECEIVE, 0x35))?;
        let _appended = credit
            .append(access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x36))?;
    }
    Ok(())
}

/// Count one unit's first step if it is the transmitting probe's frame to
/// the station.
fn count(bytes: &[u8], station: [u8; 6], counted: &mut Counted) {
    let Some((to, from)) = addresses(bytes) else {
        return;
    };
    if to != station {
        return;
    }
    let header = oer_chips_esp32s31_hal::layouts::WifiRxControlPrefix::SIZE
        + oer_chips_esp32s31_hal::layouts::WifiRxControlTail::SIZE;
    let Some(frame) = bytes.get(header..) else {
        return;
    };
    if frame.first() != Some(&ACTION) || frame.get(24) != Some(&CATEGORY) {
        return;
    }
    let Some(value) = frame
        .get(COUNTER..COUNTER + 4)
        .and_then(|word| word.try_into().ok())
        .map(u32::from_le_bytes)
    else {
        return;
    };
    counted.to_station = counted.to_station.saturating_add(1);
    counted.sender = from;
    match frame.len() {
        // With or without the FCS the walker may keep after the frame.
        30 | 34 => counted.short = counted.short.saturating_add(1),
        1_024 | 1_028 => counted.long = counted.long.saturating_add(1),
        _ => counted.other_length = counted.other_length.saturating_add(1),
    }
    match value & CLASSES {
        SECOND => {
            counted.second = counted.second.saturating_add(1);
            let chain = (value & !CLASSES) as usize;
            if let Some(word) = counted.second_seen.get_mut(chain / 32) {
                *word |= 1 << (chain % 32);
            }
        }
        OBSERVED => counted.observed = counted.observed.saturating_add(1),
        PULSE => counted.pulses = counted.pulses.saturating_add(1),
        _ => counted.stops = counted.stops.saturating_add(1),
    }
}
