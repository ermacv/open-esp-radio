//! `tasks/219` (a) and (b), `tasks/220` (probe-only, branch
//! `probe/rx-rebind-4`, never merged): what the receive walker writes after
//! its stop edge, and after `RST_WIFIMAC`, under traffic.
//!
//! The receiver of s31-b's two kinds of air (`stream`: 500-octet frames back
//! to back; `stream-ampdu`: aggregates of 16 QoS subframes under a receive
//! Block Ack agreement, programmed here once the sender is seen). Each stop:
//!
//! 1. a receive window, its air named by the last unit of the sender;
//! 2. a delay spread over a frame's time, the cursor frozen, the buffer
//!    `NEXT` names digested;
//! 3. **(a)** the walker's edge alone (`experiment_walker_edge`, the ROM's
//!    `hal_mac_rx_disable`), its readback logged and deciding nothing;
//!    **(b)** the block's reset through the domain's
//!    `reset_wifi_mac_block`, `WALKER_ENABLE` after it logged and not
//!    written;
//! 4. records of every descriptor's three words and every buffer's prefix
//!    (the RX-control prefix and tail, 64 bytes) and frame region apart,
//!    from the edge's write (a) or the reset's proof in hand (b), at
//!    [`DELAYS_US`]; each record is compared with the one before;
//! 5. the MAC's channel stopped, the walker's stop proof, the ring
//!    rebuilt; (b) the cold preparation and the channel again; a start as
//!    the first, its frames checked by their counters.
//!
//! The claim: from the first record on, no frame region and no descriptor
//! word changes. A prefix's change is reported apart. No stop ends the run
//! early: every one is a row.
use super::*;
use crate::case_c::{Case, byte};
use oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::cursor_ring_index;

/// Stops of each kind of air, for each part.
const PER_AIR: u32 = 1_000;
/// The parts' rows: two parts, two kinds of air, and a margin for stops on
/// an air of neither kind.
const ROWS_MAX: usize = 4 * PER_AIR as usize + 512;
/// The run's longest time: the stand's lease.
const LIMIT_S: u64 = 3_000;
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
/// The RX-control prefix and tail in front of an MPDU in a buffer.
const HEADER: usize = 64;

/// A part of the run.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum Part {
    Edge = 1,
    Reset = 2,
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
/// - `2`: cycles from the edge's write to its readback (a), the pulse's
///   cycles (b);
/// - `3`: the first record's start and end, microseconds from the edge,
///   start << 16 | end;
/// - `4`: for each record from the second on, 3 bits (frame, prefix,
///   descriptor) changed since the record before, record k at 3 (k - 1);
/// - `5`: nodes whose frame region changed after the first record << 16 |
///   nodes whose prefix changed after it;
/// - `6`: frames of the sender checked after the restart | bad << 16;
/// - `7`: units received after the restart.
type Row = [u32; 8];

#[derive(bytemuck::Zeroable)]
pub(super) struct Rows {
    rows: [Row; ROWS_MAX],
    used: usize,
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
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(LIMIT_S as u32))
        .ok_or((phase::RING, 0x5e01))?;
    let mut before = Record::new();
    let mut record = Record::new();
    for part in [Part::Edge, Part::Reset] {
        mark(110 + part as u32);
        let mut counts = [0_u32; 3];
        let mut index = 0_u32;
        while (counts[0] < PER_AIR || counts[1] < PER_AIR) && EmbassyClock.now() < end {
            let (_units, air) =
                window(storage, &mut ring, &mut access, &mut series, WINDOW_MS).await?;
            if let Some(count) = counts.get_mut(air as usize) {
                *count = count.saturating_add(1);
            }
            let mut row: Row = [0; 8];
            row[0] = (part as u32) << 28 | (air as u32) << 24 | (index & 0x00ff_ffff);
            wait_micros(u64::from(spread.next(SPREAD_US))).await;
            let addresses = addresses(storage).map_err(|code| (phase::RING, 0x5e10 | code))?;
            let frozen = ring
                .freeze_cursor(access.rx_dma())
                .map_err(|_| (phase::RING, 0x5e02))?;
            let base = ring.descriptor_base();
            let next = cursor_ring_index(frozen.next_descriptor_low(), base, COUNT);
            let last = cursor_ring_index(frozen.last_descriptor_low(), base, COUNT);
            let next_before = next.map(|node| digest_frame(&addresses, node));
            // The edge or the reset; `start` is the cycle the records count
            // from.
            let (start, readback) = match part {
                Part::Edge => {
                    let edge = oer_portable_riscv::cycle32();
                    access.rx_dma().experiment_walker_edge();
                    let enabled = access.receive_dma_snapshot().walker_enabled;
                    let read = oer_portable_riscv::cycle32();
                    row[2] = read.wrapping_sub(edge);
                    (edge, enabled)
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
                    (proof, access.receive_dma_snapshot().walker_enabled)
                }
            };
            let mut mask = 0_u32;
            let mut frame_nodes = 0_u32;
            let mut prefix_nodes = 0_u32;
            for (k, delay) in DELAYS_US.iter().enumerate() {
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
            // The stop proper and the rebuild, after the records.
            access.wifi_mac_hal().request_channel_stop();
            let halted = match ring.try_stop(access.rx_dma()) {
                Ok(halted) => halted,
                Err((_live, _)) => return Err((phase::WALKER_STOP, 0x5e00_0000 | index)),
            };
            ring = match part {
                Part::Edge => restart_after_stop(storage, halted, &mut access, mac_irq, power_irq)?,
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
            // A start as the first: its frames by their counters.
            let (checked, bad, units) =
                check_window(storage, &mut ring, &mut access, &mut series, WINDOW_MS).await?;
            row[6] = checked & 0xffff | bad << 16;
            row[7] = units;
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
fn note(series: &mut Series, access: &mut RadioAccess, bytes: &[u8]) -> Option<Air> {
    let mpdu = bytes.get(HEADER..)?;
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

/// Whether a steady frame of the sender holds the sender's bytes, its
/// counter in every 64 octets one frame's.
fn content_ok(series: &Series, bytes: &[u8]) -> Option<bool> {
    let sender = series.sender?;
    let mpdu = bytes.get(HEADER..)?;
    if mpdu.first() != Some(&0x08) || mpdu.get(10..16) != Some(&sender[..]) {
        return None;
    }
    let at = Case::Provenance700.counter_at();
    let counter = mpdu
        .get(at..at + 4)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_le_bytes)?;
    let length = STEADY.min(mpdu.len());
    Some((0..length).all(|offset| {
        mpdu.get(offset).copied()
            == Some(byte(
                Case::Provenance700,
                sender,
                series.station,
                counter,
                offset,
            ))
    }))
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
        if let Ok(step) = unit.step(0) {
            seen(access, step.bytes());
        }
        let credit = unit.retain_discard().ok_or((phase::RECEIVE, 0x5e45))?;
        let _appended = credit
            .append(access.rx_dma())
            .map_err(|_| (phase::RECEIVE, 0x5e46))?;
    }
    Ok(units)
}
