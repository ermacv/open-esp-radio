//! Part two's transmit half (`tasks/144` A, with `tasks/149`, `151` and
//! `152`): a frame of the probe's own on the air, its acknowledgement by the
//! stand's peer (observed at the probe, in the queue's completion), an
//! ordinary stop with that frame on the air, and the MAC's block reset with
//! it on the air followed by a second run that transmits.
//!
//! A chain the MAC held at a pulse is never returned: the HAL returns a
//! chain only by a `MacTxQueueDetached`, which compares the queue's
//! descriptor address with the chain's, and the pulse resets that address.
//! So each pulse sends from a descriptor and a buffer of their own, left
//! with the HAL for the rest of the run, and the probe reads them (their
//! three words and the frame's bytes) after the bounded wait and again at
//! the run's end. Between the abandoned buffers the pool keeps guard slots
//! of its own (`[guard, frame, guard, …]`), painted once and read with them.
use super::*;
use oer_chips_esp32s31_hal::{
    layouts::{WifiRxControlPrefix, WifiRxControlTail, WifiTxDmaDescriptor},
    radio::ieee80211::dma::{
        space::ChipDma,
        tx_storage::{PinnedTxDmaStorage, TxDmaAccess, TxDmaStorage},
    },
    system::memory::dma_static::{PlacedBacking, PlacedTxLease, PlacedTxPool},
    types::{
        MacInterface, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
        MacLegacyTxResponse, MacTxAbortCause, MacTxControlFrame, MacTxDetachOutcome,
        MacTxDetachReason, MacTxProtection, MacTxQueueIndex,
    },
};
use oer_portable_memory::{FrameLen, PinnedDmaTxPool, PinnedDmaTxRadioLease};

/// The frames' MPDU bytes, FCS excluded: about 1.4 ms on the air at
/// 6 Mb/s, the time a pulse or a stop is spread over.
const FRAME: usize = 1_024;
/// The pulses that land on a frame of their own, each chain abandoned.
const ABANDONED: usize = 100;
/// The pool's slots: a guard before, between and after the abandoned
/// frames, then the ordinary frames' one slot.
const SLOTS: usize = 2 * ABANDONED + 2;
/// The ordinary frames' slot.
const ORDINARY_SLOT: u8 = (SLOTS - 1) as u8;
/// The frames sent before the stops, each awaited.
const BASELINE: usize = 20;
/// Their lengths, in turn: an authentication frame's, then longer.
const BASELINE_LENGTHS: [usize; 4] = [30, 100, 512, FRAME];
/// The stops with a frame on the air.
pub(super) const STOPS: usize = 1_000;
/// How long a frame's completion is awaited.
const COMPLETION_MS: u32 = 20;
/// The window a pulse or a stop is spread over after the queue's start:
/// the frame's air time and a little of its contention before it.
const SPREAD_US: u32 = 1_600;
/// The bounded wait after a pulse before its chain is read again.
const ABANDON_WAIT_MS: u32 = 20;
/// The transmit queue every frame goes on.
const QUEUE: MacTxQueueIndex = MacTxQueueIndex::Zero;
/// The records part two prints.
pub(super) const ROWS: usize = 4 + BASELINE + STOPS + ABANDONED + 2;

type Pool = PinnedDmaTxPool<FRAME, 0, 0, SLOTS>;
type Placed = PlacedTxPool<FRAME, 0, 0, SLOTS>;
type Lease = PlacedTxLease<PinnedDmaTxRadioLease<'static, FRAME, 0, 0>>;
type Descriptor = PinnedTxDmaStorage<Lease>;

macro_rules! abandoned_descriptors {
    ($($name:ident)*) => {
        oer_chips_esp32s31_hal::dma_static! {
            $(static $name: TxDmaStorage = zeroed in ".dma.bss.mac_reset_tx_abandoned";)*
        }
        /// One descriptor for each pulse's chain.
        static ABANDONED_DESCRIPTORS: [&oer_chips_esp32s31_hal::system::memory::dma_static::DmaStatic<TxDmaStorage>; ABANDONED] = [$(&$name),*];
    };
}
abandoned_descriptors! {
    A00 A01 A02 A03 A04 A05 A06 A07 A08 A09 A10 A11 A12 A13 A14 A15 A16 A17 A18 A19
    A20 A21 A22 A23 A24 A25 A26 A27 A28 A29 A30 A31 A32 A33 A34 A35 A36 A37 A38 A39
    A40 A41 A42 A43 A44 A45 A46 A47 A48 A49 A50 A51 A52 A53 A54 A55 A56 A57 A58 A59
    A60 A61 A62 A63 A64 A65 A66 A67 A68 A69 A70 A71 A72 A73 A74 A75 A76 A77 A78 A79
    A80 A81 A82 A83 A84 A85 A86 A87 A88 A89 A90 A91 A92 A93 A94 A95 A96 A97 A98 A99
}
/// Where the pinned pool lives for the run.
static POOL_PLACE: static_cell::StaticCell<Placed> = static_cell::StaticCell::new();
oer_chips_esp32s31_hal::dma_static! {
    static TX_POOL: Pool = zeroed in ".dma.bss.mac_reset_tx_pool";
    static TX_ORDINARY: TxDmaStorage = zeroed in ".dma.bss.mac_reset_tx_ordinary";
}

/// The phases part two's failures name.
mod tx_phase {
    pub const POOL: u32 = 30;
    pub const PEER: u32 = 31;
    pub const FRAME: u32 = 32;
    pub const START: u32 = 33;
    pub const COMPLETION: u32 = 34;
    pub const RELEASE: u32 = 35;
    pub const STOP: u32 = 36;
    pub const PULSE: u32 = 37;
}

/// The single site that reads the probe's own placed DMA memory while the
/// HAL holds it (`tasks/152`): the word `offset` bytes into the `len` bytes
/// at `start`, or `None` past them or off a word.
#[expect(
    unsafe_code,
    reason = "tasks/152: the probe's one read of its placed DMA memory while the HAL holds it"
)]
fn read_placed_word(start: u32, len: u32, offset: u32) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let address = start.checked_add(offset)?;
    if end > len || address % 4 != 0 {
        return None;
    }
    let pointer = core::ptr::with_exposed_provenance::<u32>(address as usize);
    // SAFETY: `start..start + len` is the probe's own placement in the DMA
    // window (a placed buffer's region or a placed descriptor's binding, a
    // field's address being its DMA address on the chip), `offset + 4` is
    // within it and the address is word aligned. The HAL holds that memory
    // for the rest of the run and lends it to no one, so nothing of this
    // program writes it; a write by the MAC's bus master is the observation
    // asked for, and a volatile read of plain words cannot tear a value
    // the program relies on.
    Some(unsafe { core::ptr::read_volatile(pointer) })
}

/// The frame byte at `offset`: a header to the peer, a vendor-specific
/// action's category and Espressif's OUI (an ESP-NOW frame's start), then
/// a pattern of the chain.
fn frame_byte(header: &[u8; 24], chain: u32, offset: usize) -> u8 {
    const SNAP: [u8; 8] = [0x7f, 0x18, 0xfe, 0x34, 0, 0, 0, 0];
    if let Some(byte) = header.get(offset) {
        return *byte;
    }
    if let Some(byte) = offset.checked_sub(24).and_then(|at| SNAP.get(at)) {
        return *byte;
    }
    (chain as u8) ^ (offset as u8) ^ 0x5a
}

/// An action frame from the station to the peer with the wildcard BSSID,
/// as ESP-NOW sends one: an unassociated ESP-IDF station takes it and its
/// MAC acknowledges it, where it drops a data frame of another BSS.
fn header(station: [u8; 6], peer: [u8; 6]) -> [u8; 24] {
    // Frame control: management, action; duration: an ACK at 6 Mb/s and SIFS.
    let start = [0xd0, 0x00, 44, 0];
    let bssid = [0xff; 6];
    core::array::from_fn(|at| match at {
        0..4 => start.get(at).copied().unwrap_or(0),
        4..10 => peer.get(at - 4).copied().unwrap_or(0),
        10..16 => station.get(at - 10).copied().unwrap_or(0),
        16..22 => bssid.get(at - 16).copied().unwrap_or(0),
        _ => 0,
    })
}

/// The guard pattern of slot `slot`'s word `word`.
const fn guard_word(slot: usize, word: usize) -> u32 {
    0xa5a5_0000 ^ ((slot as u32) << 8) ^ (word as u32)
}

/// The legacy program of one frame: the backend's direct-MPDU profile
/// (`LegacyTxConfig::management`) at 6 Mb/s, answered by an ACK.
fn parameters(length: usize) -> MacLegacyTxParameters {
    MacLegacyTxParameters {
        control: MacTxControlFrame {
            protection: MacTxProtection::None,
            rate: MacLegacyRate::Ofdm6M,
            power_primary: 0,
            power_alternate: 0,
        },
        rate: MacLegacyRate::Ofdm6M,
        // The PSDU: the MPDU and the FCS the hardware appends.
        signal: (length + 4) as u16,
        data_power: 8,
        timeout: 0x03ff,
        scheduler_priority: 1,
        packet_priority: 1,
        priority_count: 1,
        aifsn: 2,
        contention_window: 0,
        interface: MacInterface::Station,
        response: MacLegacyTxResponse::Ack,
        hardware_key_selector: 0,
    }
}

/// The pool and its guard slots, held for the whole run.
struct Frames {
    pool: &'static Placed,
    guards: heapless::Vec<
        oer_portable_memory::PinnedDmaTxNetworkLease<'static, FRAME, 0, 0>,
        { ABANDONED + 1 },
    >,
    header: [u8; 24],
}

impl Frames {
    /// The slot of abandoned chain `chain`, between guards `chain` and
    /// `chain + 1` (slots `2 chain` and `2 chain + 2`).
    const fn frame_slot(chain: usize) -> u8 {
        (2 * chain + 1) as u8
    }

    /// Paint every guard slot with its pattern, holding it for the run.
    fn paint_guards(&mut self) -> Result<(), Failure> {
        for guard in 0..=ABANDONED {
            let slot = (2 * guard) as u8;
            let mut lease = self
                .pool
                .claim_network(slot)
                .map_err(|_| (tx_phase::POOL, u32::from(slot)))?;
            for (word, bytes) in lease.bytes_mut().chunks_exact_mut(4).enumerate() {
                bytes.copy_from_slice(&guard_word(usize::from(slot), word).to_le_bytes());
            }
            self.guards
                .push(lease)
                .map_err(|_| (tx_phase::POOL, 0x100 | u32::from(slot)))?;
        }
        Ok(())
    }

    /// The guard slots' words that differ from their pattern.
    fn changed_guard_words(&mut self, guards: core::ops::Range<usize>) -> u32 {
        let mut changed = 0_u32;
        for guard in guards {
            let Some(lease) = self.guards.get_mut(guard) else {
                continue;
            };
            let slot = usize::from(lease.index());
            for (word, bytes) in lease.bytes_mut().chunks_exact(4).enumerate() {
                let mut read = [0_u8; 4];
                for (cell, byte) in read.iter_mut().zip(bytes) {
                    *cell = oer_portable_memory::read_volatile(byte);
                }
                if u32::from_le_bytes(read) != guard_word(slot, word) {
                    changed = changed.saturating_add(1);
                }
            }
        }
        changed
    }

    /// Slot `slot`'s frame of chain `chain`, as the radio holds it.
    fn lease(&self, slot: u8, chain: u32, length: usize) -> Result<Lease, Failure> {
        let header = self.header;
        let length = FrameLen::new(length).map_err(|_| (tx_phase::FRAME, 0))?;
        let network = self
            .pool
            .claim_network(slot)
            .map_err(|_| (tx_phase::FRAME, 1 | u32::from(slot) << 8))?;
        let _published = network.publish(length, |bytes| {
            for (offset, byte) in bytes.iter_mut().enumerate() {
                *byte = frame_byte(&header, chain, offset);
            }
        });
        self.pool
            .claim_radio(slot)
            .map_err(|_| (tx_phase::FRAME, 2 | u32::from(slot) << 8))
    }
}

/// A descriptor with its frame attached, reserved and its queue started.
fn start(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    lease: Lease,
    length: usize,
) -> Result<(), Failure> {
    descriptor.attach(lease).map_err(|_| (tx_phase::START, 1))?;
    let TxDmaAccess::Free(free) = descriptor.access() else {
        return Err((tx_phase::START, 2));
    };
    free.reserve(FRAME as u32, length as u32)
        .map_err(|_| (tx_phase::START, 3))?;
    let publication = descriptor.publication().map_err(|_| (tx_phase::START, 4))?;
    let program =
        MacLegacyTxProgram::new(&publication, parameters(length)).ok_or((tx_phase::START, 5))?;
    let mut hal = access.wifi_mac_hal();
    hal.prepare_bound_legacy_tx(&publication, QUEUE, program)
        .map_err(|_| (tx_phase::START, 6))?;
    hal.start_bound_tx(publication, QUEUE, || {})
        .map_err(|_| (tx_phase::START, 7))?;
    Ok(())
}

/// How one ordinary frame ended: its completion's status and detail, or
/// how the queue gave it back.
#[derive(Clone, Copy)]
struct Ended {
    /// 0 completed, 1 aborted and detached, 2 completed after the abort
    /// found it on the air.
    how: u32,
    status: u32,
    detail: u32,
}

/// The frame's end after a stop: its completion if it had one, else the
/// abort and the queue's detach. `None` when the abort found the frame on
/// the air: its completion comes once the MAC runs again
/// ([`await_completion`]).
fn abort(access: &mut RadioAccess, descriptor: &mut Descriptor) -> Result<Option<Ended>, Failure> {
    if let Some(observation) = access.wifi_mac_hal().take_tx_completion(QUEUE) {
        return complete(access, descriptor, observation, 0).map(Some);
    }
    let _forced = access
        .wifi_mac_hal()
        .begin_tx_timeout_abort(QUEUE, MacTxAbortCause::Forced);
    match access.wifi_mac_hal().with_tx_queue_detached(
        QUEUE,
        MacTxDetachReason::Timeout,
        |detached| descriptor.release_aborted(detached),
    ) {
        MacTxDetachOutcome::Detached(Ok(())) => {
            let _lease = descriptor.detach();
            Ok(Some(Ended {
                how: 1,
                status: 0,
                detail: 0,
            }))
        }
        MacTxDetachOutcome::OnAir => Ok(None),
        MacTxDetachOutcome::Detached(Err(_)) => Err((tx_phase::RELEASE, 1)),
        MacTxDetachOutcome::NoEvent | MacTxDetachOutcome::Failed => Err((tx_phase::RELEASE, 2)),
    }
}

/// Await the queue's completion for at most [`COMPLETION_MS`]; the
/// descriptor is free again and its lease back in the pool. `how` is
/// recorded: 0 an ordinary completion, 2 one an abort found on the air.
async fn await_completion(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    how: u32,
) -> Result<Ended, Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(COMPLETION_MS))
        .ok_or((tx_phase::COMPLETION, 0))?;
    let observation = loop {
        if let Some(observation) = access.wifi_mac_hal().take_tx_completion(QUEUE) {
            break observation;
        }
        if EmbassyClock.now() >= end {
            return Err((tx_phase::COMPLETION, 1 | how << 4));
        }
        wait_micros(50).await;
    };
    complete(access, descriptor, observation, how)
}

/// A completion taken: the descriptor released by the queue's detach.
fn complete(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    observation: oer_chips_esp32s31_hal::types::MacTxCompletionObservation,
    how: u32,
) -> Result<Ended, Failure> {
    descriptor
        .mark_completed()
        .map_err(|_| (tx_phase::RELEASE, 3))?;
    match access.wifi_mac_hal().with_tx_queue_detached(
        QUEUE,
        MacTxDetachReason::Completed,
        |detached| descriptor.release_completed(detached),
    ) {
        MacTxDetachOutcome::Detached(Ok(())) => {}
        _ => return Err((tx_phase::RELEASE, 4)),
    }
    let _lease = descriptor.detach();
    Ok(Ended {
        how,
        status: u32::from(observation.status()),
        detail: u32::from(observation.detail()),
    })
}

/// One ordinary frame sent and awaited: how it ended, and the cycles from
/// its start to its completion.
async fn send(
    access: &mut RadioAccess,
    frames: &Frames,
    ordinary: &mut Descriptor,
    chain: u32,
    length: usize,
) -> Result<(Ended, u32), Failure> {
    let lease = frames.lease(ORDINARY_SLOT, chain, length)?;
    let begin = oer_portable_riscv::cycle32();
    start(access, ordinary, lease, length)?;
    let ended = await_completion(access, ordinary, 0).await?;
    Ok((ended, oer_portable_riscv::cycle32().wrapping_sub(begin)))
}

/// The peer: the sender of the first frame addressed to the station, read
/// from the ring within two seconds.
async fn learn_peer(
    storage: &Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    station: [u8; 6],
) -> Result<[u8; 6], Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(2))
        .ok_or((tx_phase::PEER, 0))?;
    while EmbassyClock.now() < end {
        storage
            .observe_reserved(ring, access.rx_dma())
            .map_err(|_| (tx_phase::PEER, 1))?;
        ring.complete_pending_reload(access.rx_dma(), Settle::new(RELOAD_SETTLE, &EmbassyClock))
            .map_err(|_| (tx_phase::PEER, 2))?;
        let frozen = ring
            .freeze_cursor(access.rx_dma())
            .map_err(|_| (tx_phase::PEER, 3))?;
        let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| (tx_phase::PEER, 4))?
        else {
            wait_micros(100).await;
            continue;
        };
        // The frame follows the control prefix and its tail; a frame with
        // HE-SIG-B options between them is not the peer's (6 Mb/s).
        let peer = unit.step(0).ok().and_then(|step| {
            let bytes = step.bytes();
            let prefix = WifiRxControlPrefix::new(bytes.first_chunk()?);
            if prefix.he_sig_b().present() {
                return None;
            }
            let frame = bytes.get(WifiRxControlPrefix::SIZE + WifiRxControlTail::SIZE..)?;
            let to: [u8; 6] = frame.get(4..10)?.try_into().ok()?;
            let from: [u8; 6] = frame.get(10..16)?.try_into().ok()?;
            (to == station).then_some(from)
        });
        let credit = unit.retain_discard().ok_or((tx_phase::PEER, 5))?;
        let _appended = credit
            .append(access.rx_dma())
            .map_err(|_| (tx_phase::PEER, 6))?;
        if let Some(peer) = peer {
            return Ok(peer);
        }
    }
    Err((tx_phase::PEER, 7))
}

/// An abandoned chain: its descriptor, left with the HAL, and what the
/// probe read of it right after its pulse.
struct Abandoned {
    descriptor: Descriptor,
    /// The descriptor's address and the buffer's region, read once.
    descriptor_address: u32,
    buffer_start: u32,
    buffer_len: u32,
    /// The descriptor's three words read right after the pulse: what the
    /// MAC wrote before or with the pulse, against which a later write is
    /// told apart.
    reference: [u32; 3],
}

impl Abandoned {
    /// The descriptor's three words now.
    fn words(&self) -> [u32; 3] {
        let len = WifiTxDmaDescriptor::SIZE as u32;
        [0, 4, 8].map(|offset| {
            read_placed_word(self.descriptor_address, len, offset).unwrap_or(u32::MAX)
        })
    }

    /// The descriptor's words that differ from their reference, and the
    /// buffer's words that differ from the frame written into it.
    fn changed(&self, header: &[u8; 24], chain: u32) -> (u32, u32) {
        let descriptor = self
            .words()
            .iter()
            .zip(self.reference)
            .filter(|(now, then)| **now != *then)
            .count() as u32;
        let mut buffer = 0_u32;
        for word in 0..FRAME / 4 {
            let offset = (word * 4) as u32;
            let expected = u32::from_le_bytes(core::array::from_fn(|byte| {
                frame_byte(header, chain, word * 4 + byte)
            }));
            if read_placed_word(self.buffer_start, self.buffer_len, offset) != Some(expected) {
                buffer = buffer.saturating_add(1);
            }
        }
        (descriptor, buffer)
    }
}

/// What part two borrows from the session.
pub(super) struct TransmitParts<'a> {
    pub(super) storage: &'static Storage,
    pub(super) slot: &'a mut Option<Radio>,
    pub(super) mac_irq: &'a mut MacInterruptRegisters,
    pub(super) power_irq: &'a mut MacPowerInterruptRegisters,
    pub(super) sources: &'a mut Sources,
    pub(super) timer: &'a CooperativeTimer,
    pub(super) mac: MacColdStartConfig,
    pub(super) channel: Ghz2_4Channel,
    pub(super) station: [u8; 6],
    pub(super) seed: u32,
}

/// Part two's transmit series over the running MAC and its live ring.
#[expect(
    clippy::too_many_lines,
    reason = "one series in the order the task states it, each step recorded where it happens"
)]
pub(super) async fn series(
    parts: TransmitParts<'_>,
    mut access: RadioAccess,
    mut clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mut ring: Ring,
    journal: &mut Journal,
) -> Result<(), Failure> {
    let TransmitParts {
        storage,
        slot,
        mac_irq,
        power_irq,
        sources,
        timer,
        mac,
        channel,
        station,
        seed,
    } = parts;
    let pool =
        Placed::pin(TX_POOL.take().ok_or((tx_phase::POOL, 1))?).map_err(|_| (tx_phase::POOL, 2))?;
    let pool: &'static Placed = POOL_PLACE.try_init(pool).ok_or((tx_phase::POOL, 5))?;
    let mut ordinary: Descriptor =
        TxDmaStorage::pin::<ChipDma, Lease>(TX_ORDINARY.take().ok_or((tx_phase::POOL, 3))?)
            .map_err(|_| (tx_phase::POOL, 4))?;

    mark(70);
    let peer = learn_peer(storage, &mut ring, &mut access, station).await?;
    journal.record(
        Kind::Peer,
        &[
            u32::from_be_bytes([0, 0, peer[0], peer[1]]),
            u32::from_be_bytes([peer[2], peer[3], peer[4], peer[5]]),
        ],
    );
    let mut frames = Frames {
        pool,
        guards: heapless::Vec::new(),
        header: header(station, peer),
    };
    frames.paint_guards()?;

    // The single frame: sent and acknowledged, before anything is stopped.
    mark(71);
    // Of four lengths in turn, the MAC's transmit counters beside each.
    for index in 0..BASELINE {
        let index32 = index as u32;
        let length = BASELINE_LENGTHS
            .get(index % BASELINE_LENGTHS.len())
            .copied()
            .unwrap_or(FRAME);
        let counted = access.wifi_mac_hal().transmit_statistics_snapshot();
        let (ended, cycles) = send(&mut access, &frames, &mut ordinary, index32, length).await?;
        let counted = access
            .wifi_mac_hal()
            .transmit_statistics_snapshot()
            .wrapping_delta_since(counted);
        journal.record(
            Kind::Tx,
            &[
                index32,
                ended.status,
                ended.detail,
                cycles,
                length as u32,
                counted.tx_rts,
                counted.tx_cts,
                counted.track,
                counted.trcts,
            ],
        );
    }

    // A3: an ordinary stop with a frame on the air.
    mark(72);
    let mut spread = Spread(seed);
    for index in 0..STOPS {
        let index32 = index as u32;
        let units = receive(storage, &mut ring, &mut access, 1 + spread.next(3))
            .await
            .map_err(|code| (phase::RECEIVE, code))?;
        let lease = frames.lease(ORDINARY_SLOT, index32, FRAME)?;
        start(&mut access, &mut ordinary, lease, FRAME)?;
        let delay = spread.next(SPREAD_US);
        wait_micros(u64::from(delay)).await;
        let queue = access.wifi_mac_hal().ordinary_tx_queue_snapshot(QUEUE);
        let begin = oer_portable_riscv::cycle32();
        access.wifi_mac_hal().request_channel_stop();
        let requested = oer_portable_riscv::cycle32();
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, index32 | 0x0100_0000)),
        };
        let bound = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(STOP_BOUND_MS))
            .ok_or((tx_phase::STOP, index32))?;
        let mut active = access.wifi_mac_hal().channel_active_state();
        while active != 0 && EmbassyClock.now() < bound {
            active = access.wifi_mac_hal().channel_active_state();
        }
        let idle = oer_portable_riscv::cycle32();
        let after = access.wifi_mac_hal().ordinary_tx_queue_snapshot(QUEUE);
        let aborted = abort(&mut access, &mut ordinary)?;
        ring = restart(storage, halted, &mut access, mac_irq, power_irq)?;
        let ended = match aborted {
            Some(ended) => ended,
            None => await_completion(&mut access, &mut ordinary, 2).await?,
        };
        journal.record(
            Kind::TxStop,
            &[
                index32,
                units,
                delay,
                requested.wrapping_sub(begin),
                idle.wrapping_sub(begin),
                u32::from(active),
                queue_flags(&queue),
                queue_flags(&after),
                ended.how,
                ended.status,
                ended.detail,
                mac_irq.mac_interrupt_raw().bits(),
            ],
        );
    }

    // A1 and A2: the pulse with a frame of its own on the air, then a
    // second run that transmits.
    mark(73);
    let mut chains: heapless::Vec<Abandoned, ABANDONED> = heapless::Vec::new();
    for (chain, place) in ABANDONED_DESCRIPTORS.iter().enumerate() {
        let chain32 = chain as u32;
        let units_before = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code | 0x0200_0000))?;
        let mut descriptor: Descriptor =
            TxDmaStorage::pin::<ChipDma, Lease>(place.take().ok_or((tx_phase::PULSE, chain32))?)
                .map_err(|_| (tx_phase::PULSE, chain32 | 0x100))?;
        let lease = frames.lease(Frames::frame_slot(chain), chain32, FRAME)?;
        let region = lease
            .dma_region()
            .map_err(|_| (tx_phase::PULSE, chain32 | 0x200))?;
        let descriptor_address = descriptor.binding().descriptor_address();
        start(&mut access, &mut descriptor, lease, FRAME)?;
        let delay = spread.next(SPREAD_US);
        wait_micros(u64::from(delay)).await;
        let before = access.wifi_mac_hal().ordinary_tx_queue_snapshot(QUEUE);
        let word0_before = descriptor.descriptor_word0().bits();
        let pulse_cycle = oer_portable_riscv::cycle32();
        let (pulsed, pulsed_clocks) = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            clocked.reset_mac(held(slot)?);
            clocked.into_running()
        };
        access = pulsed;
        clocks_on = pulsed_clocks;
        let mut abandoned = Abandoned {
            descriptor,
            descriptor_address,
            buffer_start: region.start().get(),
            buffer_len: region.len(),
            reference: [0; 3],
        };
        abandoned.reference = abandoned.words();
        let after = access.wifi_mac_hal().ordinary_tx_queue_snapshot(QUEUE);
        let active = access.wifi_mac_hal().channel_active_state();
        let raw = mac_irq.mac_interrupt_raw().bits();
        // The vendor's order, as part one: the walker stopped at once, the
        // cold preparation, the channel, the start.
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, chain32 | 0x0400_0000)),
        };
        let second_cold = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let result = {
                let mut hal = clocked.cold_mac_hal(held(slot)?);
                initialize_wifi_mac(sources, &mut hal, timer, mac).await
            };
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        second_cold.map_err(|_| (phase::SECOND_COLD, chain32 | 0x0400_0000))?;
        let retuned = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let mut platform = ();
            let result = apart!(tune(slot, &mut clocked, timer, &mut platform, channel));
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        retuned.map_err(|(phase, code)| (phase, code | 0x0400_0000 | chain32 << 8))?;
        ring = restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
        // The rest of the bounded wait, then the chain and its guards read.
        let waited = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(ABANDON_WAIT_MS))
            .ok_or((tx_phase::PULSE, chain32 | 0x300))?;
        let units_second = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code | 0x0800_0000))?;
        if EmbassyClock.now() < waited {
            EmbassyClock.wait_until(waited).await;
        }
        let (descriptor_changed, buffer_changed) = abandoned.changed(&frames.header, chain32);
        let guards_changed = frames.changed_guard_words(chain..chain + 2);
        // The second run transmits.
        let (second, second_cycles) = send(
            &mut access,
            &frames,
            &mut ordinary,
            0x8000_0000 | chain32,
            FRAME,
        )
        .await?;
        journal.record(
            Kind::TxPulse,
            &[
                chain32,
                units_before,
                delay,
                pulse_cycle,
                queue_flags(&before),
                word0_before,
                queue_flags(&after),
                after.descriptor_address_low,
                abandoned.reference[0],
                abandoned.reference[1],
                abandoned.reference[2],
                u32::from(active),
                raw,
                descriptor_changed,
                buffer_changed,
                guards_changed,
                second.status,
                second.detail,
                second_cycles,
                units_second,
            ],
        );
        chains
            .push(abandoned)
            .map_err(|_| (tx_phase::PULSE, chain32 | 0x400))?;
    }

    // Every abandoned chain and every guard read once more, after every
    // later pulse, cold preparation, start and frame.
    mark(74);
    let mut descriptors = 0_u32;
    let mut buffers = 0_u32;
    let mut first = u32::MAX;
    for (chain, abandoned) in chains.iter().enumerate() {
        let (descriptor, buffer) = abandoned.changed(&frames.header, chain as u32);
        if (descriptor != 0 || buffer != 0) && first == u32::MAX {
            first = chain as u32;
        }
        descriptors = descriptors.saturating_add(descriptor);
        buffers = buffers.saturating_add(buffer);
    }
    let guards = frames.changed_guard_words(0..ABANDONED + 1);
    journal.record(
        Kind::Abandoned,
        &[chains.len() as u32, descriptors, buffers, guards, first],
    );
    // Every owner stays held until the leased host reset; the abandoned
    // chains are the HAL's for good.
    let _held = (access, clocks_on, ring, ordinary, chains, frames);
    Ok(())
}

/// A queue's flags: bit 0 valid, 1 enabled, 2 a completion pending, 3 a
/// timeout pending, 4 a collision pending.
fn queue_flags(
    snapshot: &oer_chips_esp32s31_hal::radio::ieee80211::registers::MacOrdinaryTxQueueSnapshot,
) -> u32 {
    u32::from(snapshot.valid)
        | u32::from(snapshot.enabled) << 1
        | u32::from(snapshot.completion_pending) << 2
        | u32::from(snapshot.timeout_pending) << 3
        | u32::from(snapshot.collision_pending) << 4
}
