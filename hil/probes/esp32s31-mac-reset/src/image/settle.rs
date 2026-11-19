//! `tasks/219`, `221` and `222` (probe-only, branch `probe/rx-rebind-4`,
//! never merged): what the receive walker writes after the MAC's own stop,
//! and after `RST_WIFIMAC` in the vendor's order, under traffic.
//!
//! The receiver of s31-b's two kinds of air (`stream`: 500-octet frames back
//! to back; `stream-ampdu`: aggregates of 16 QoS subframes under a receive
//! Block Ack agreement, programmed here once the sender is seen). Each stop:
//!
//! 1. a receive window, its air named by the last unit of the sender;
//! 2. a delay spread over a frame's time, the cursor frozen, the buffer
//!    `NEXT` names digested;
//! 3. **(a2)** the MAC's stop as the channel switch has it
//!    (`request_channel_stop`, `hal_mac_deinit`), the activity field read
//!    until clear (a status the MAC sets), then the walker's edge
//!    (`experiment_walker_edge`, the ROM's `hal_mac_rx_disable`); the
//!    records count from the activity's clear. **The reset** in the
//!    vendor's order (`libnet80211[wifi_reset_mac]`): the block's reset
//!    through the domain's `reset_wifi_mac_block`, then the walker's edge;
//!    the records count from the reset's proof. `WALKER_ENABLE` read after
//!    it is logged and decides nothing;
//! 4. records of every descriptor's three words and every buffer's prefix
//!    (the RX-control prefix and tail, 64 bytes) and frame region apart, at
//!    [`DELAYS_US`] from that moment; each record is compared with the one
//!    before;
//! 5. the walker's stop proof, the ring rebuilt, the MAC resumed (a2) or
//!    the cold preparation and the channel again (the reset); a start as
//!    the first, the sender's frames checked by their payload.
//!
//! Each part has its own budget ([`A2_BUDGET_S`], [`RESET_BUDGET_S`]) and
//! ends when it made [`PER_AIR`] stops of each kind of air or its budget
//! ran out; `short` makes every budget a short pass's.
//!
//! The claim: from the first record on, no frame region and no descriptor
//! word changes. A prefix's change is reported apart. No stop ends the run
//! early: every one is a row.
use super::*;
use crate::case_c::Case;
use oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::cursor_ring_index;

/// Stops of each kind of air, for each part.
const PER_AIR: u32 = 1_000;
/// The parts' rows: two parts, two kinds of air, and a margin for stops on
/// an air of neither kind.
const ROWS_MAX: usize = 3_500;
/// Each part's budget (`tasks/222`: the two under fifty minutes).
#[cfg(not(feature = "short"))]
const A2_BUDGET_S: u32 = 11 * 60;
#[cfg(not(feature = "short"))]
const RESET_BUDGET_S: u32 = 25 * 60;
/// The short pass of a lease (`tasks/222`): about a minute in all.
#[cfg(feature = "short")]
const A2_BUDGET_S: u32 = 30;
#[cfg(feature = "short")]
const RESET_BUDGET_S: u32 = 30;
/// S1's budget, each order (`tasks/227`, `228`): (a3) ours and the vendor's.
#[cfg(not(feature = "short"))]
const A3_BUDGET_S: u32 = 12 * 60;
/// S2's budget, each variant: two of the sender's turn pairs of 150 s.
#[cfg(not(feature = "short"))]
const R_BUDGET_S: u32 = 11 * 60;
#[cfg(feature = "short")]
const R_BUDGET_S: u32 = 30;
/// R4's and R5's budgets (`tasks/233`): ten minutes each.
#[cfg(not(feature = "short"))]
const R45_BUDGET_S: u32 = 10 * 60;
#[cfg(feature = "short")]
const R45_BUDGET_S: u32 = 30;
/// R5's wait after the stop atom, on the timer (`tasks/232`).
const RELEASE_WAIT_US: u64 = 1_000;
/// `tasks/257`'s turns: the two kinds alternate, at least two hundred each.
const T257_TURNS: u32 = 440;
#[cfg(not(feature = "short"))]
const T257_BUDGET_S: u32 = 8 * 60;
#[cfg(feature = "short")]
const T257_BUDGET_S: u32 = 30;
/// `tasks/257`'s settle after the stop request (`tasks/256`: the request,
/// 20 us, one read).
const T257_SETTLE_US: u32 = 20;
/// The longest wait for the first data success after a start, in cycles
/// (200 ms).
const FIRST_SUCCESS_BOUND: u32 = 200_000 * CYCLES_PER_US;
/// Every cycle's short row (`tasks/233`).
const SHORT_MAX: usize = 30_000;
/// The vendor's settle after the stop request, before the activity's poll
/// (SOURCE: `libpp.a[hal_mac.o]::hal_mac_deinit`, `tasks/227`).
const VENDOR_REQUEST_SETTLE_US: u32 = 20;
/// The vendor's settle after the activity's clear (same source).
const VENDOR_CLEAR_SETTLE_US: u32 = 5;
/// S1's second sum, from the activity's clear (`tasks/228`).
const SECOND_SUM_US: u32 = 20;
/// The bound of `hal_deinit`'s handshake wait (the vendor has none).
const HANDSHAKE_BOUND_US: u32 = 10_000;
#[cfg(feature = "short")]
const A3_BUDGET_S: u32 = 30;
/// (a3)'s quick copy: NEXT's node and the one after it, which fit in about
/// 20 us (a node's digest about 12.6 us in run 2's first record).
const QUICK: usize = 2;
/// The longest wait for the sender's first frame before a part's budget
/// starts.
const SENDER_WAIT_S: u32 = 240;
/// The longest wait for the MAC's activity to clear after its stop.
const ACTIVITY_BOUND_US: u32 = 10_000;
/// The records' delays from the edge or the reset's proof.
const DELAYS_US: [u32; 5] = [20, 100, 1_000, 20_000, 200_000];
/// `mcycle` at 320 MHz.
const CYCLES_PER_US: u32 = 320;
/// The prefix's words: the 56-byte RX-control prefix and its 8-byte tail.
const PREFIX_WORDS: usize = 16;
/// A stop lands at a point spread over a frame's time (a 500-octet frame
/// back to back, an aggregate's subframes).
const SPREAD_US: u32 = 1_500;
/// The receive window before a stop and after a start.
const WINDOW_MS: u32 = 20;
/// The frame's length of the steady air (`stream`).
const STEADY: usize = 500;

/// A part of the run.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum Part {
    /// The MAC's stop, the activity's clear, the walker's edge.
    A2 = 3,
    /// The block's reset, then the walker's edge.
    Reset = 4,
    /// S1, our order (`tasks/228`): (a2) with the sums of the buffers the
    /// unit in flight can touch, at the activity's clear and 20 us after it.
    A3 = 5,
    /// S1, the vendor's order: the walker's edge, the stop request, 20 us,
    /// the poll, 5 us; the same sums.
    Vendor = 6,
    /// S2: our stop (as (a2)), the walker's stop proof, then the reset and
    /// the cold preparation; no records.
    R1 = 7,
    /// S2: R1 with `hal_deinit`'s edges before the reset.
    R2 = 8,
    /// S2: the vendor's whole stop (its order, then `hal_deinit`'s edges),
    /// then the reset.
    R3 = 9,
    /// `tasks/233`, the last resort's shape: the walker's edge and the stop
    /// request, then the reset at once, with no settle and no poll.
    R4 = 10,
    /// `tasks/233`: the whole stop in the order the port carries (the
    /// walker's edge, the stop atom with its 20 us and 5 us, a wait of
    /// 1 ms), then the reset.
    R5 = 11,
    /// `tasks/257`: even turns stop with the reset (the walker's edge, the
    /// stop request, 20 us, the reset) and start cold; odd turns stop with
    /// the wait (the walker's edge, the stop request, 20 us, one read of
    /// the activity, 1 ms on the timer) and start warm. Every step stamped.
    T257 = 12,
}

impl Part {
    const fn budget_s(self) -> u32 {
        match self {
            Self::A2 => A2_BUDGET_S,
            Self::Reset => RESET_BUDGET_S,
            Self::A3 | Self::Vendor => A3_BUDGET_S,
            Self::R1 | Self::R2 | Self::R3 => R_BUDGET_S,
            Self::R4 | Self::R5 => R45_BUDGET_S,
            Self::T257 => T257_BUDGET_S,
        }
    }

    /// S2's parts run their whole budget, through the sender's turns.
    const fn resets(self) -> bool {
        matches!(self, Self::R1 | Self::R2 | Self::R3 | Self::R4 | Self::R5)
    }

    /// The walker's edge before the stop request.
    const fn walker_first(self) -> bool {
        matches!(self, Self::Vendor | Self::R3 | Self::R4 | Self::R5)
    }

    /// The stop atom's settles of 20 us and 5 us.
    const fn vendor(self) -> bool {
        matches!(self, Self::Vendor | Self::R3 | Self::R5)
    }

    /// S1's sums.
    const fn sums(self) -> bool {
        matches!(self, Self::A3 | Self::Vendor)
    }
}

/// The air a stop fell in, by the last unit of the sender before it.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum Air {
    Steady = 0,
    Aggregate = 1,
    Other = 2,
}

/// One stop's row.
///
/// - `0`: part << 28 | air << 24 | index;
/// - `1`: NEXT's ring index | LAST's << 8 | flags << 16 (bit 0 NEXT's buffer
///   changed between the freeze and the first record, bit 1 NEXT's
///   descriptor completed at the first record, bit 2 the walker read
///   enabled after the edge or the reset, bit 3 NEXT outside the ring);
/// - `2`: (a2) cycles from the stop's request to the activity's clear, bit
///   31 set when it did not clear within [`ACTIVITY_BOUND_US`]; (reset) the
///   pulse's cycles;
/// - `3`: the first record's start and end, microseconds from the edge,
///   start << 16 | end;
/// - `4`: for each record from the second on, 3 bits (frame, prefix,
///   descriptor) changed since the record before, record k at 3 (k - 1);
/// - `5`: nodes whose frame region changed after the first record << 16 |
///   nodes whose prefix changed after it;
/// - `6`: frames of the sender checked after the restart | bad << 16;
/// - `7`: units received after the restart | the sender's frames among
///   them << 16;
/// - `8`: over the window after the start, the MAC's MPDU counter's advance
///   << 16 | its data-success counter's (`tasks/223`: what the receiver
///   acknowledged; run 200 saw it still while no ACK went);
/// - `9` (`tasks/223`): the receive filter's words
///   (`experiment_receive_filter_words`) against the first start's: which
///   of the eight differ before the stop | after the start << 8, and
///   `AUTO_ACK_DISABLE` of the four policies before << 16 | after << 20.
///   `10` (S1): cycles from the activity's clear to the first sum's end
///   | its nodes << 24; `11` (S1): for the sums' node j (NEXT + j), 3 bits
///   (frame, prefix, descriptor) changed between the first sum and the
///   second at 3 j, and between the second and the first record at 6 + 3 j;
///   the second sum's start << 12 and end << 22, microseconds from the
///   clear (10 bits each).
///
/// S2's rows (`R1` to `R3`) take no records: `3`, `4` NEXT and LAST read
/// between the reset and the cold preparation; `5`, `11` NEXT and LAST
/// after the cold preparation and the channel, before the walker's start;
/// `10` the cycles of `hal_deinit`'s handshake wait | bit 31 when it did not
/// clear | bit 30 LAST zero and bit 29 NEXT zero just after the start.
///
/// `12` to `16` (`tasks/231`, S2's rows, read after the start): the
/// baseband watchdog's status, control and enable words; the MAC's
/// activity field (bit 0 transmitting, bit 1 receiving) | the interrupt
/// `ENABLE` as the HAL names it << 8 (0 disabled, 1 the station's, 2 other)
/// | the walker's enable << 16 | the reload's pending bit << 17; the
/// interrupt `RAW` word.
///
/// `tasks/257`'s rows (`T257`), cycles from the walker's edge, zero for a
/// step the turn has not: `2` the stop request, `3` the settle's end, `4`
/// the read of the activity (wait), `5` the wait's end (wait), `10` the
/// walker's stop proof, `11` the reset's request (cold), `12` its proof
/// (cold), `13` the cold preparation's end (cold), `14` the channel's end
/// (cold), `15` the ring published, `16` the receive policy and the
/// interrupt mask set, `17` the walker enabled, `18` the MAC's receive
/// released ("ready"); `19`: bit 0 the wait's turn (else the reset's), bit 1
/// the activity read set, bit 2 no data success within 200 ms; `20` the
/// first data success after the start.
type Row = [u32; 21];

/// `tasks/257`'s stamps, by step.
const T_EDGE: usize = 0;
const T_REQUEST: usize = 1;
const T_SETTLED: usize = 2;
const T_READ: usize = 3;
const T_WAITED: usize = 4;
const T_PROOF: usize = 5;
const T_PULSE: usize = 6;
const T_RESET: usize = 7;
const T_COLD: usize = 8;
const T_CHANNEL: usize = 9;
const T_RING: usize = 10;
const T_POLICY: usize = 11;
const T_ENABLE: usize = 12;
const T_READY: usize = 13;
const T_FIRST: usize = 14;
const T257_STAMPS: usize = 15;

/// The first start's filter words and the first sets that differ from
/// them after a start.
const DIFFERING: usize = 16;
/// The silent starts read again.
const SILENT: usize = 64;

#[derive(bytemuck::Zeroable)]
pub(super) struct Rows {
    rows: [Row; ROWS_MAX],
    used: usize,
    /// The first silent starts of S2 read again after their window: the
    /// row's word `0`, `Row`'s `12` to `16`, NEXT, LAST.
    pub(super) silent: [[u32; 8]; SILENT],
    pub(super) silent_used: usize,
    /// `tasks/231`: for each part (3 to 9) and air, the windows before a
    /// stop in which each `RAW` bit was set.
    pub(super) raw_counts: [[[u32; 32]; 3]; 9],
    /// The windows each count of `raw_counts` is out of.
    pub(super) raw_windows: [[u32; 3]; 9],
    /// `tasks/233`: every cycle's short row: the row's word `0`; units
    /// after the start | the data successes << 16 (15 bits) | silent << 31.
    pub(super) short: [[u32; 2]; SHORT_MAX],
    pub(super) short_used: usize,
    /// Stops past [`ROWS_MAX`], counted and not kept.
    pub(super) dropped: u32,
    pub(super) reference: [u32; 8],
    pub(super) differing: [(u32, [u32; 8]); DIFFERING],
    pub(super) differing_used: usize,
}

pub(super) static ROWS: oer_portable_memory::zeroed::ZeroedStatic<Rows> =
    oer_portable_memory::zeroed::ZeroedStatic::new();
pub(super) static ROWS_DONE: oer_portable_memory::ClaimCell<&'static mut Rows> =
    oer_portable_memory::ClaimCell::new();

impl Rows {
    fn push(&mut self, row: Row) {
        if let Some(cell) = self.rows.get_mut(self.used) {
            *cell = row;
            self.used = self.used.saturating_add(1);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    pub(super) fn rows(&self) -> &[Row] {
        self.rows.get(..self.used).unwrap_or(&[])
    }
}

/// One record of the arena: each node's prefix and frame digests and its
/// descriptor's words.
#[derive(Clone, Copy)]
struct Record {
    prefix: [u64; NODES],
    frame: [u64; NODES],
    words: [[u32; 3]; NODES],
}

impl Record {
    const fn new() -> Self {
        Self {
            prefix: [0; NODES],
            frame: [0; NODES],
            words: [[0; 3]; NODES],
        }
    }
}

/// What a record found changed against the one before.
#[derive(Clone, Copy, Default)]
struct Changed {
    frame: u32,
    prefix: u32,
    descriptor: u32,
}

fn changed(before: &Record, after: &Record) -> Changed {
    let mut changed = Changed::default();
    for node in 0..NODES {
        changed.frame += u32::from(before.frame.get(node) != after.frame.get(node));
        changed.prefix += u32::from(before.prefix.get(node) != after.prefix.get(node));
        changed.descriptor += u32::from(before.words.get(node) != after.words.get(node));
    }
    changed
}

/// The state a series keeps between its stops.
struct Series {
    station: [u8; 6],
    sender: Option<[u8; 6]>,
    agreement_peer: Option<[u8; 6]>,
    /// The receive agreement programmed since the last cold preparation.
    agreement: bool,
}

/// The diagnostic words of `Row`'s `12` to `16` (`tasks/231`).
fn diagnostics(
    slot: &mut Option<Radio>,
    access: &mut RadioAccess,
    mac_irq: &MacInterruptRegisters,
) -> Result<[u32; 5], Failure> {
    let [status, control, enable] = held(slot)?.experiment_baseband_watchdog_words();
    let dma = access.receive_dma_snapshot();
    let mask = match mac_irq.mac_interrupt_enable() {
        oer_chips_esp32s31_hal::radio::ieee80211::registers::MacInterruptEnableState::Disabled => 0,
        oer_chips_esp32s31_hal::radio::ieee80211::registers::MacInterruptEnableState::Station => 1,
        oer_chips_esp32s31_hal::radio::ieee80211::registers::MacInterruptEnableState::Unknown => 2,
    };
    Ok([
        status,
        control,
        enable,
        u32::from(access.wifi_mac_hal().channel_active_state())
            | mask << 8
            | u32::from(dma.walker_enabled) << 16
            | u32::from(dma.reload_pending) << 17,
        mac_irq.mac_interrupt_raw().bits(),
    ])
}

/// Parts (a) and (b), each until [`PER_AIR`] stops of both kinds of air.
#[allow(clippy::too_many_arguments, reason = "the series' owners")]
pub(super) async fn series(
    storage: &'static Storage,
    slot: &mut Option<Radio>,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
    sources: &mut Sources,
    timer: &CooperativeTimer,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
    seed: u32,
    mut access: RadioAccess,
    mut clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mut ring: Ring,
    journal: &mut Journal,
) -> Result<(), Failure> {
    let Some(rows) = ROWS.try_take() else {
        return Err((phase::RING, 0x5e00));
    };
    let mut spread = Spread(seed);
    let mut series = Series {
        station: mac.station_address,
        sender: None,
        agreement_peer: None,
        agreement: false,
    };
    let mut before = Record::new();
    let mut record = Record::new();
    rows.reference = access.wifi_mac_hal().experiment_receive_filter_words();
    // `tasks/224`: one part a boot, so that no reset comes before (a2).
    #[cfg(feature = "only-a2")]
    let parts = [Part::A2];
    #[cfg(feature = "only-reset")]
    let parts = [Part::Reset];
    #[cfg(feature = "only-a3")]
    let parts = [Part::A3];
    #[cfg(feature = "only-vendor")]
    let parts = [Part::Vendor];
    #[cfg(feature = "only-r1")]
    let parts = [Part::R1];
    #[cfg(feature = "only-r2")]
    let parts = [Part::R2];
    #[cfg(feature = "only-r3")]
    let parts = [Part::R3];
    #[cfg(feature = "only-r4")]
    let parts = [Part::R4];
    #[cfg(feature = "only-r5")]
    let parts = [Part::R5];
    #[cfg(feature = "only-t257")]
    let parts = [Part::T257];
    #[cfg(not(any(
        feature = "only-a2",
        feature = "only-reset",
        feature = "only-a3",
        feature = "only-vendor",
        feature = "only-r1",
        feature = "only-r2",
        feature = "only-r3",
        feature = "only-r4",
        feature = "only-r5",
        feature = "only-t257"
    )))]
    let parts = [Part::A2, Part::Reset];
    for part in parts {
        mark(110 + part as u32);
        // `tasks/224`, lease 1's short pass: the sender's first frame comes
        // about 80 s after the receiver's start, so a part's budget runs
        // from it, not from the boot.
        let wait_end = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(SENDER_WAIT_S))
            .ok_or((phase::RING, 0x5e03))?;
        loop {
            let (_units, air) =
                window(storage, &mut ring, &mut access, &mut series, WINDOW_MS).await?;
            if air != Air::Other || EmbassyClock.now() >= wait_end {
                break;
            }
        }
        let end = EmbassyClock
            .now()
            .checked_add(Duration::from_secs(part.budget_s()))
            .ok_or((phase::RING, 0x5e01))?;
        let mut counts = [0_u32; 3];
        let mut index = 0_u32;
        while (part.resets() || part == Part::T257 || counts[0] < PER_AIR || counts[1] < PER_AIR)
            && !(part == Part::T257 && index >= T257_TURNS)
            && EmbassyClock.now() < end
        {
            // `tasks/257`'s stamps, cycles: see `Row`.
            let mut stamps = [0_u32; T257_STAMPS];
            let cold_turn = index % 2 == 0;
            let (_units, air) =
                window(storage, &mut ring, &mut access, &mut series, WINDOW_MS).await?;
            if let Some(count) = counts.get_mut(air as usize) {
                *count = count.saturating_add(1);
            }
            let mut row: Row = [0; 21];
            row[0] = (part as u32) << 28 | (air as u32) << 24 | (index & 0x00ff_ffff);
            wait_micros(u64::from(spread.next(SPREAD_US))).await;
            let before_stop = access.wifi_mac_hal().experiment_receive_filter_words();
            // `tasks/231`: the causes latched since the start (`open_epoch`
            // acknowledges every one), counted by bit.
            let raw = mac_irq.mac_interrupt_raw().bits();
            if let Some(windows) = rows
                .raw_windows
                .get_mut(part as usize - 3)
                .and_then(|kinds| kinds.get_mut(air as usize))
            {
                *windows = windows.saturating_add(1);
            }
            if let Some(counts) = rows
                .raw_counts
                .get_mut(part as usize - 3)
                .and_then(|kinds| kinds.get_mut(air as usize))
            {
                for (bit, count) in counts.iter_mut().enumerate() {
                    *count = count.saturating_add(raw >> bit & 1);
                }
            }
            let addresses = addresses(storage).map_err(|code| (phase::RING, 0x5e10 | code))?;
            let frozen = ring
                .freeze_cursor(access.rx_dma())
                .map_err(|_| (phase::RING, 0x5e02))?;
            let base = ring.descriptor_base();
            let next = cursor_ring_index(frozen.next_descriptor_low(), base, COUNT);
            let last = cursor_ring_index(frozen.last_descriptor_low(), base, COUNT);
            let next_before = next.map(|node| digest_frame(&addresses, node));
            let mut first = [(0_u64, 0_u64, [0_u32; 3]); QUICK];
            let mut second = first;
            // The edge or the reset; `start` is the cycle the records count
            // from.
            let (start, readback) = match part {
                Part::T257 => {
                    let edge = oer_portable_riscv::cycle32();
                    access.rx_dma().experiment_walker_edge();
                    stamps[T_REQUEST] = oer_portable_riscv::cycle32();
                    access.wifi_mac_hal().request_channel_stop();
                    let bound = T257_SETTLE_US.saturating_mul(CYCLES_PER_US);
                    while oer_portable_riscv::cycle32().wrapping_sub(stamps[T_REQUEST]) < bound {}
                    stamps[T_SETTLED] = oer_portable_riscv::cycle32();
                    if !cold_turn {
                        let active = access.wifi_mac_hal().channel_active_state() != 0;
                        stamps[T_READ] = oer_portable_riscv::cycle32();
                        row[19] |= u32::from(active) << 1;
                        wait_micros(RELEASE_WAIT_US).await;
                        stamps[T_WAITED] = oer_portable_riscv::cycle32();
                    }
                    stamps[T_EDGE] = edge;
                    (edge, access.receive_dma_snapshot().walker_enabled)
                }
                Part::R4 => {
                    // The last resort's shape: no settle, no poll; the reset
                    // follows in the cold preparation below.
                    access.rx_dma().experiment_walker_edge();
                    let request = oer_portable_riscv::cycle32();
                    access.wifi_mac_hal().request_channel_stop();
                    row[2] = u32::from(access.wifi_mac_hal().channel_active_state() != 0) << 30;
                    (request, access.receive_dma_snapshot().walker_enabled)
                }
                Part::A2 | Part::A3 | Part::Vendor | Part::R1 | Part::R2 | Part::R3 | Part::R5 => {
                    if part.walker_first() {
                        access.rx_dma().experiment_walker_edge();
                    }
                    let request = oer_portable_riscv::cycle32();
                    access.wifi_mac_hal().request_channel_stop();
                    if part.vendor() {
                        at_delay(request, VENDOR_REQUEST_SETTLE_US).await;
                    }
                    let bound = ACTIVITY_BOUND_US.saturating_mul(CYCLES_PER_US);
                    let mut cleared = access.wifi_mac_hal().channel_active_state() == 0;
                    while !cleared && oer_portable_riscv::cycle32().wrapping_sub(request) < bound {
                        cleared = access.wifi_mac_hal().channel_active_state() == 0;
                    }
                    let clear = oer_portable_riscv::cycle32();
                    row[2] = clear.wrapping_sub(request) | u32::from(!cleared) << 31;
                    if !part.walker_first() {
                        access.rx_dma().experiment_walker_edge();
                    }
                    if part.sums()
                        && let Some(at) = next
                    {
                        // The vendor's 5 us pass inside the first sum, which
                        // only reads.
                        sums(&addresses, storage, at, &mut first);
                        let summed = oer_portable_riscv::cycle32();
                        row[10] = summed.wrapping_sub(clear) & 0x00ff_ffff | (QUICK as u32) << 24;
                        at_delay(clear, SECOND_SUM_US).await;
                        let began = oer_portable_riscv::cycle32();
                        sums(&addresses, storage, at, &mut second);
                        let ended = oer_portable_riscv::cycle32();
                        let micros =
                            |cycle: u32| (cycle.wrapping_sub(clear) / CYCLES_PER_US).min(0x3ff);
                        row[11] =
                            sum_bits(&first, &second) | micros(began) << 12 | micros(ended) << 22;
                    } else if part.vendor() {
                        at_delay(clear, VENDOR_CLEAR_SETTLE_US).await;
                    }
                    if part == Part::R5 {
                        wait_micros(RELEASE_WAIT_US).await;
                    }
                    (clear, access.receive_dma_snapshot().walker_enabled)
                }
                Part::Reset => {
                    let pulse = oer_portable_riscv::cycle32();
                    let (pulsed, pulsed_clocks) = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let _reset =
                            held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                        clocked.into_running()
                    };
                    access = pulsed;
                    clocks_on = pulsed_clocks;
                    let proof = oer_portable_riscv::cycle32();
                    row[2] = proof.wrapping_sub(pulse);
                    // The vendor's order: the walker's edge after the reset.
                    access.rx_dma().experiment_walker_edge();
                    (proof, access.receive_dma_snapshot().walker_enabled)
                }
            };
            let mut mask = 0_u32;
            let mut frame_nodes = 0_u32;
            let mut prefix_nodes = 0_u32;
            // S2 takes no records: its question is the start after the reset.
            let delays: &[u32] = if part.resets() || part == Part::T257 {
                &[]
            } else {
                &DELAYS_US
            };
            row[1] = next.map_or(0xff, |n| n as u32) & 0xff
                | (last.map_or(0xff, |n| n as u32) & 0xff) << 8
                | u32::from(readback) << 18;
            for (k, delay) in delays.iter().enumerate() {
                at_delay(start, *delay).await;
                let began = oer_portable_riscv::cycle32();
                take_record(&addresses, storage, &mut record);
                let ended = oer_portable_riscv::cycle32();
                if k == 0 {
                    row[3] = (began.wrapping_sub(start) / CYCLES_PER_US) << 16
                        | (ended.wrapping_sub(start) / CYCLES_PER_US) & 0xffff;
                    let mut flags = 0_u32;
                    if let (Some(node), Some(digest)) = (next, next_before) {
                        flags |= u32::from(record.frame.get(node) != Some(&digest));
                        flags |=
                            u32::from(node_of(storage, node).is_some_and(|d| d.word0().rx_done()))
                                << 1;
                    }
                    flags |= u32::from(readback) << 2;
                    flags |= u32::from(next.is_none()) << 3;
                    if part.sums()
                        && let Some(at) = next
                    {
                        row[11] |= record_bits(&record, at, &second) << 6;
                    }
                    row[1] = next.map_or(0xff, |n| n as u32) & 0xff
                        | (last.map_or(0xff, |n| n as u32) & 0xff) << 8
                        | flags << 16;
                } else {
                    let seen = changed(&before, &record);
                    let bits = u32::from(seen.frame != 0)
                        | u32::from(seen.prefix != 0) << 1
                        | u32::from(seen.descriptor != 0) << 2;
                    mask |= bits << (3 * (k - 1));
                    frame_nodes = frame_nodes.max(seen.frame);
                    prefix_nodes = prefix_nodes.max(seen.prefix);
                }
                before = record;
            }
            row[4] = mask;
            row[5] = frame_nodes << 16 | prefix_nodes;
            // The walker's stop proof and the rebuild, after the records.
            let halted = match ring.try_stop(access.rx_dma()) {
                Ok(halted) => halted,
                Err((_live, _)) => return Err((phase::WALKER_STOP, 0x5e00_0000 | index)),
            };
            stamps[T_PROOF] = oer_portable_riscv::cycle32();
            ring = match part {
                Part::T257 if cold_turn => {
                    let cold = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let result = {
                            stamps[T_PULSE] = oer_portable_riscv::cycle32();
                            let reset =
                                held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                            stamps[T_RESET] = oer_portable_riscv::cycle32();
                            let mut hal = clocked.cold_mac_hal();
                            let prepared =
                                initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await;
                            stamps[T_COLD] = oer_portable_riscv::cycle32();
                            prepared
                        };
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    cold.map_err(|_| (phase::SECOND_COLD, 0x5e00_0000 | index))?;
                    let retuned = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let mut platform = ();
                        let result =
                            apart!(tune(slot, &mut clocked, timer, &mut platform, channel));
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    retuned.map_err(|(phase, code)| (phase, code | 0x5e00_0000))?;
                    stamps[T_CHANNEL] = oer_portable_riscv::cycle32();
                    series.agreement = false;
                    // `restart_after_cold`, step by step: the ring published
                    // into the gated core, the gates opened, published again.
                    let gated = storage
                        .prepare_halted(halted, access.rx_dma())
                        .map_err(|_| (phase::SECOND_RING, 0x0800_0003))?;
                    access.wifi_mac_hal().reopen_mac_core(&gated);
                    let stopped = storage
                        .prepare_halted(gated.into_halted(), access.rx_dma())
                        .map_err(|_| (phase::SECOND_RING, 0x0800_0000))?;
                    stamps[T_RING] = oer_portable_riscv::cycle32();
                    start_stamped(&mut stamps, stopped, &mut access, mac_irq, power_irq)?
                }
                Part::T257 => {
                    // The warm start: `restart_after_stop`, step by step.
                    let stopped = storage
                        .prepare_halted(halted, access.rx_dma())
                        .map_err(|_| (phase::RESTART, 0x5e20))?;
                    stamps[T_RING] = oer_portable_riscv::cycle32();
                    start_stamped(&mut stamps, stopped, &mut access, mac_irq, power_irq)?
                }
                // The MAC resumed by the start (`start_mac_runtime`).
                Part::A2 | Part::A3 | Part::Vendor => {
                    restart_after_stop(storage, halted, &mut access, mac_irq, power_irq)?
                }
                Part::R1 | Part::R2 | Part::R3 | Part::R4 | Part::R5 => {
                    if matches!(part, Part::R2 | Part::R3) {
                        // `hal_deinit`'s edges, in its order.
                        access.wifi_mac_hal().experiment_hal_deinit_words();
                        mac_irq.experiment_hal_deinit_interrupts();
                        access.wifi_mac_hal().experiment_hal_deinit_request();
                        let asked = oer_portable_riscv::cycle32();
                        let bound = HANDSHAKE_BOUND_US.saturating_mul(CYCLES_PER_US);
                        let mut ready = access.wifi_mac_hal().experiment_handshake_ready();
                        while ready && oer_portable_riscv::cycle32().wrapping_sub(asked) < bound {
                            ready = access.wifi_mac_hal().experiment_handshake_ready();
                        }
                        row[10] = oer_portable_riscv::cycle32().wrapping_sub(asked) & 0x00ff_ffff
                            | u32::from(ready) << 31;
                    }
                    let cold = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let result = {
                            let reset =
                                held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                            // S3: the walker's registers after the reset.
                            let after_reset = clocked.experiment_rx_dma_snapshot();
                            row[3] = after_reset.next_descriptor_low;
                            row[4] = after_reset.last_descriptor_low;
                            let mut hal = clocked.cold_mac_hal();
                            initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await
                        };
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    cold.map_err(|_| (phase::SECOND_COLD, 0x5e00_0000 | index))?;
                    let retuned = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let mut platform = ();
                        let result =
                            apart!(tune(slot, &mut clocked, timer, &mut platform, channel));
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    retuned.map_err(|(phase, code)| (phase, code | 0x5e00_0000))?;
                    series.agreement = false;
                    // S3: after the cold preparation, before the walker.
                    let before_start = access.receive_dma_snapshot();
                    row[5] = before_start.next_descriptor_low;
                    row[11] = before_start.last_descriptor_low;
                    let started =
                        restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
                    let after_start = access.receive_dma_snapshot();
                    row[10] |= u32::from(after_start.last_descriptor_low == 0) << 30
                        | u32::from(after_start.next_descriptor_low == 0) << 29;
                    started
                }
                Part::Reset => {
                    let cold = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let result = {
                            let reset =
                                held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                            let mut hal = clocked.cold_mac_hal();
                            initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await
                        };
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    cold.map_err(|_| (phase::SECOND_COLD, 0x5e00_0000 | index))?;
                    let retuned = {
                        let mut clocked = WifiClocked::from_running(access, clocks_on);
                        let mut platform = ();
                        let result =
                            apart!(tune(slot, &mut clocked, timer, &mut platform, channel));
                        let (running, on) = clocked.into_running();
                        access = running;
                        clocks_on = on;
                        result
                    };
                    retuned.map_err(|(phase, code)| (phase, code | 0x5e00_0000))?;
                    series.agreement = false;
                    restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?
                }
            };
            if part == Part::T257 {
                // The first data success after the start, the sender on air.
                let before = statistics(&access)[1];
                let begun = oer_portable_riscv::cycle32();
                let mut seen = false;
                while !seen
                    && oer_portable_riscv::cycle32().wrapping_sub(begun) < FIRST_SUCCESS_BOUND
                {
                    seen = statistics(&access)[1] != before;
                }
                stamps[T_FIRST] = oer_portable_riscv::cycle32();
                row[19] |= u32::from(!cold_turn) | u32::from(!seen) << 2;
                let edge = stamps[T_EDGE];
                for (cell, stamp) in [2_usize, 3, 4, 5, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20]
                    .into_iter()
                    .zip(&stamps[1..])
                {
                    if let Some(cell) = row.get_mut(cell) {
                        *cell = if *stamp == 0 {
                            0
                        } else {
                            stamp.wrapping_sub(edge)
                        };
                    }
                }
            }
            // A start as the first: its frames by their counters.
            let after_start = access.wifi_mac_hal().experiment_receive_filter_words();
            row[9] = filter_row(&rows.reference, &before_stop, &after_start);
            if after_start != rows.reference && rows.differing_used < DIFFERING {
                if let Some(slot) = rows.differing.get_mut(rows.differing_used) {
                    *slot = (row[0], after_start);
                }
                rows.differing_used += 1;
            }
            if part.resets() {
                let words = diagnostics(slot, &mut access, mac_irq)?;
                for (cell, word) in row.iter_mut().skip(12).zip(words) {
                    *cell = word;
                }
            }
            let counted = statistics(&access);
            let (checked, bad, units) =
                check_window(storage, &mut ring, &mut access, &mut series, WINDOW_MS).await?;
            row[8] = advance(counted, statistics(&access), 0, 1);
            row[6] = checked & 0xffff | bad << 16;
            row[7] = units & 0xffff | checked << 16;
            // `tasks/231`: a silent start read once more after its window.
            if part.resets() && row[7] >> 16 != 0 && row[8] & 0xffff == 0 {
                let words = diagnostics(slot, &mut access, mac_irq)?;
                let dma = access.receive_dma_snapshot();
                if let Some(cell) = rows.silent.get_mut(rows.silent_used) {
                    *cell = [
                        row[0],
                        words[0],
                        words[1],
                        words[2],
                        words[3],
                        words[4],
                        dma.next_descriptor_low,
                        dma.last_descriptor_low,
                    ];
                }
                rows.silent_used = rows.silent_used.saturating_add(1);
            }
            let silent = row[7] >> 16 != 0 && row[8] & 0xffff == 0;
            if let Some(cell) = rows.short.get_mut(rows.short_used) {
                *cell = [
                    row[0],
                    row[7] & 0xffff | (row[8] & 0x7fff) << 16 | u32::from(silent) << 31,
                ];
                rows.short_used = rows.short_used.saturating_add(1);
            }
            rows.push(row);
            index = index.saturating_add(1);
        }
        journal.record(
            Kind::End,
            &[part as u32, index, counts[0], counts[1], counts[2]],
        );
    }
    let _ = ROWS_DONE.install(rows);
    Ok(())
}

/// The sums of S1 (`tasks/228`): NEXT's node and the one after it, each
/// buffer's prefix and frame region by a word-wise sum and xor ([`digest`])
/// and the descriptor's three words.
fn sums(
    addresses: &[usize; NODES],
    storage: &Storage,
    at: usize,
    out: &mut [(u64, u64, [u32; 3]); QUICK],
) {
    for (j, slot) in out.iter_mut().enumerate() {
        let node = (at + j) % COUNT;
        if let Some(address) = addresses.get(node) {
            *slot = (
                digest(*address, 0, PREFIX_WORDS),
                digest(*address, PREFIX_WORDS, WORDS),
                node_of(storage, node).map_or([0; 3], |d| {
                    [d.word0().bits(), d.buffer_address(), d.next_address()]
                }),
            );
        }
    }
}

/// For each node of two sums, 3 bits (frame, prefix, descriptor) changed.
fn sum_bits(before: &[(u64, u64, [u32; 3]); QUICK], after: &[(u64, u64, [u32; 3]); QUICK]) -> u32 {
    before
        .iter()
        .zip(after)
        .enumerate()
        .fold(0, |bits, (j, ((p0, f0, w0), (p1, f1, w1)))| {
            bits | (u32::from(f0 != f1) | u32::from(p0 != p1) << 1 | u32::from(w0 != w1) << 2)
                << (3 * j)
        })
}

/// For each node of a sum, 3 bits changed between it and `record`.
fn record_bits(record: &Record, at: usize, sum: &[(u64, u64, [u32; 3]); QUICK]) -> u32 {
    sum.iter()
        .enumerate()
        .fold(0, |bits, (j, (prefix, frame, words))| {
            let node = (at + j) % COUNT;
            bits | (u32::from(record.frame.get(node) != Some(frame))
                | u32::from(record.prefix.get(node) != Some(prefix)) << 1
                | u32::from(record.words.get(node) != Some(words)) << 2)
                << (3 * j)
        })
}

/// An ordinary start after a stop: the ring rebuilt, the MAC's core
/// reopened, the open receive, the epoch, the walker, the MAC released, as
/// part one's stops do.
fn restart_after_stop(
    storage: &'static Storage,
    halted: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingHalted<'static, COUNT>,
    access: &mut RadioAccess,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
) -> Result<Ring, Failure> {
    let stopped = storage
        .prepare_halted(halted, access.rx_dma())
        .map_err(|_| (phase::RESTART, 0x5e20))?;
    access.wifi_mac_hal().reopen_mac_core(&stopped);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    station_policy(access);
    open_epoch(mac_irq, power_irq);
    let ring = stopped
        .try_start(access.rx_dma())
        .map_err(|_| (phase::RESTART, 0x5e21))?;
    access.wifi_mac_hal().start_mac_runtime(&ring);
    Ok(ring)
}

/// The warm or cold start's last steps, stamped (`tasks/257`): the MAC's
/// core reopened, the receive policy and the interrupt mask, the walker
/// enabled, the MAC's receive released.
fn start_stamped(
    stamps: &mut [u32; T257_STAMPS],
    stopped: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingStopped<'static, COUNT>,
    access: &mut RadioAccess,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
) -> Result<Ring, Failure> {
    access.wifi_mac_hal().reopen_mac_core(&stopped);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    station_policy(access);
    open_epoch(mac_irq, power_irq);
    stamps[T_POLICY] = oer_portable_riscv::cycle32();
    let ring = stopped
        .try_start(access.rx_dma())
        .map_err(|_| (phase::RESTART, 0x5e21))?;
    stamps[T_ENABLE] = oer_portable_riscv::cycle32();
    access.wifi_mac_hal().start_mac_runtime(&ring);
    stamps[T_READY] = oer_portable_riscv::cycle32();
    Ok(ring)
}

/// The filter words of one stop against the first start's (`Row`'s `9`).
fn filter_row(reference: &[u32; 8], before: &[u32; 8], after: &[u32; 8]) -> u32 {
    let mask = |words: &[u32; 8]| {
        words
            .iter()
            .zip(reference)
            .enumerate()
            .fold(0_u32, |m, (i, (w, r))| m | u32::from(w != r) << i)
    };
    let ack = |words: &[u32; 8]| {
        words
            .iter()
            .take(4)
            .enumerate()
            .fold(0_u32, |m, (i, w)| m | (w >> 17 & 1) << i)
    };
    mask(before) | mask(after) << 8 | ack(before) << 16 | ack(after) << 20
}

/// Wait until `delay` microseconds after the cycle `start`: by the cycle
/// counter up to a millisecond, by the clock beyond it.
async fn at_delay(start: u32, delay: u32) {
    let target = delay.saturating_mul(CYCLES_PER_US);
    if delay > 1_000 {
        let now = oer_portable_riscv::cycle32().wrapping_sub(start);
        if let Some(left) = target.checked_sub(now) {
            wait_micros(u64::from(left / CYCLES_PER_US)).await;
        }
    }
    while oer_portable_riscv::cycle32().wrapping_sub(start) < target {}
}

/// Each node's buffer address, from its descriptor before the stop.
fn addresses(storage: &Storage) -> Result<[usize; NODES], u32> {
    let mut addresses = [0; NODES];
    for (node, address) in addresses.iter_mut().enumerate() {
        *address = buffer_address(storage, node)?;
    }
    Ok(addresses)
}

fn node_of(storage: &Storage, node: usize) -> Option<&RxDescriptor> {
    if node == COUNT {
        Some(storage.reserved_descriptor())
    } else {
        storage.descriptors().get(node)
    }
}

/// A digest of words `from..to` of the buffer at `address`.
#[allow(
    unsafe_code,
    reason = "probe-only volatile read of the probe's own DMA arena"
)]
fn digest(address: usize, from: usize, to: usize) -> u64 {
    let mut sum = 0_u32;
    let mut mixed = 0_u32;
    for word in from..to {
        // SAFETY: `address` is a buffer of the probe's own static arena,
        // checked by `buffer_address` against the arena and the buffer's
        // capacity; `word` lies below its words. A load, no reference.
        let value = unsafe { (address as *const u32).add(word).read_volatile() };
        sum = sum.wrapping_add(value);
        mixed = mixed.rotate_left(5) ^ value;
    }
    u64::from(sum) << 32 | u64::from(mixed)
}

fn digest_frame(addresses: &[usize; NODES], node: usize) -> u64 {
    addresses
        .get(node)
        .map_or(0, |address| digest(*address, PREFIX_WORDS, WORDS))
}

fn take_record(addresses: &[usize; NODES], storage: &Storage, record: &mut Record) {
    oer_portable_riscv::fence();
    for node in 0..NODES {
        let Some(address) = addresses.get(node) else {
            continue;
        };
        if let Some(prefix) = record.prefix.get_mut(node) {
            *prefix = digest(*address, 0, PREFIX_WORDS);
        }
        if let Some(frame) = record.frame.get_mut(node) {
            *frame = digest(*address, PREFIX_WORDS, WORDS);
        }
        if let (Some(words), Some(descriptor)) =
            (record.words.get_mut(node), node_of(storage, node))
        {
            *words = [
                descriptor.word0().bits(),
                descriptor.buffer_address(),
                descriptor.next_address(),
            ];
        }
    }
}

/// The air of one unit of the sender, and the peer it names; the
/// agreement programmed once its peer is seen.
fn note(series: &mut Series, access: &mut RadioAccess, mpdu: &[u8]) -> Option<Air> {
    let kind = *mpdu.first()?;
    let to = mpdu.get(4..10)?;
    let from = <[u8; 6]>::try_from(mpdu.get(10..16)?).ok()?;
    if to != series.station {
        return None;
    }
    match kind {
        0x88 => {
            series.agreement_peer.get_or_insert(from);
            if !series.agreement
                && let Some(peer) = series.agreement_peer
            {
                use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::hardware::{
                    S31RxBlockAckAgreement, program,
                };
                let _programmed = program(
                    &mut access.wifi_mac_hal(),
                    S31RxBlockAckAgreement {
                        hardware_index: 0,
                        interface: oer_chips_esp32s31_hal::types::MacInterface::Station,
                        peer,
                        tid: 0,
                        starting_sequence:
                            oer_protocols_ieee80211_wire::sequence::SequenceNumber::ZERO,
                        window: 64,
                    },
                );
                series.agreement = true;
            }
            Some(Air::Aggregate)
        }
        0x08 => {
            series.sender.get_or_insert(from);
            Some(Air::Steady)
        }
        _ => None,
    }
}

/// Whether a steady frame of the sender holds the sender's payload, its
/// counter in every 64 octets one frame's (`case_c::payload_ok`: the header
/// is the MAC's in part and not compared).
fn content_ok(series: &Series, mpdu: &[u8]) -> Option<bool> {
    crate::case_c::payload_ok(
        Case::Provenance700,
        mpdu,
        series.sender?,
        series.station,
        STEADY,
    )
}

/// The MPDU of a unit's first step: where the RX-control prefix and its
/// optional blocks end (`first_segment_layout`, the backend's reading of the
/// prefix), for the frame's length.
fn mpdu_of<'a>(
    step: &oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_storage::RxUnitStep<'a>,
) -> Option<&'a [u8]> {
    use oer_chips_esp32s31_radio_ieee80211_backend::mac::rx::{
        RxIngressConfig, first_segment_layout,
    };
    let bytes = step.bytes();
    let view = oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxSegment {
        descriptor_address: 0,
        descriptor_word0: step.descriptor_word0(),
        buffer: bytes,
        next_descriptor_address: 0,
    };
    let layout = first_segment_layout(
        &view,
        RxIngressConfig {
            ring_entry_limit: COUNT,
            csi_config: 0,
            flags: 0,
        },
    )
    .ok()?;
    let end = layout
        .frame_offset
        .checked_add(layout.expected_frame_length)?;
    bytes.get(layout.frame_offset..end.min(bytes.len()))
}

/// Receive for `millis`, taking and recycling every unit; the units and
/// the air of the sender's last one.
async fn window(
    storage: &Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    series: &mut Series,
    millis: u32,
) -> Result<(u32, Air), Failure> {
    let mut air = Air::Other;
    let units = drain(storage, ring, access, millis, |access, bytes| {
        if let Some(seen) = note(series, access, bytes) {
            air = seen;
        }
    })
    .await?;
    Ok((units, air))
}

/// Receive for `millis` after a start: the sender's steady frames checked
/// by content; checked, bad, units.
async fn check_window(
    storage: &Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    series: &mut Series,
    millis: u32,
) -> Result<(u32, u32, u32), Failure> {
    let mut checked = 0_u32;
    let mut bad = 0_u32;
    let units = drain(storage, ring, access, millis, |access, bytes| {
        let _ = note(series, access, bytes);
        match content_ok(series, bytes) {
            Some(true) => checked = checked.saturating_add(1),
            Some(false) => {
                checked = checked.saturating_add(1);
                bad = bad.saturating_add(1);
            }
            None => {}
        }
    })
    .await?;
    Ok((checked, bad, units))
}

/// Take and recycle every completed unit for `millis`, showing each one's
/// first step to `seen`.
async fn drain(
    storage: &Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    millis: u32,
    mut seen: impl FnMut(&mut RadioAccess, &[u8]),
) -> Result<u32, Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(millis))
        .ok_or((phase::RECEIVE, 0x5e40))?;
    let mut units = 0_u32;
    while EmbassyClock.now() < end {
        storage
            .observe_reserved(ring, access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x5e41))?;
        ring.complete_pending_reload(access.rx_dma(), Settle::new(RELOAD_SETTLE, &EmbassyClock))
            .map_err(|_| (phase::RECEIVE, 0x5e42))?;
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x5e43))?;
        let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| (phase::RECEIVE, 0x5e44))?
        else {
            wait_micros(50).await;
            continue;
        };
        units = units.saturating_add(1);
        if let Ok(step) = unit.step(0)
            && let Some(mpdu) = mpdu_of(&step)
        {
            seen(access, mpdu);
        }
        let credit = unit.retain_discard().ok_or((phase::RECEIVE, 0x5e45))?;
        let _appended = credit
            .append(access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x5e46))?;
    }
    Ok(units)
}
