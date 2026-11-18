//! Cell F of `tasks/161`, its first figure (`tasks/173`): the control alone,
//! QoS Data without A-MSDU, in A-MPDUs of 2 and then of 8 subframes, through
//! the backend's A-MPDU owner (`RetainedDmaAmpduTx`, built as the station
//! builds it), unprotected, to the receiving rx-chain probe, which holds a
//! receive Block Ack agreement for TID 0 programmed through the backend.
//!
//! Every subframe carries case C's control layout with its index in the run
//! as its counter and its own sequence number; the Block Ack the receiver
//! returns is kept bit by bit per subframe, so that it can be read against
//! what the receiver completed. First [`HELLO`] single frames give the
//! receiver this board's address; the probe draws no backoff.
use super::amsdu::AmsduParts;
use super::transmit::{Lease, ORDINARY_SLOT, TIMEOUT, await_completion, start, tx_phase};
use super::*;
use crate::case_c::{CELL_F_AMSDU_FRAMES, CELL_F_FRAMES, CELL_F_OCTETS, CELL_F_PAIRS, Case, byte};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::dma::tx_ampdu_storage::AmpduDmaStorage, system::memory::dma_static::Placed,
};
use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::{
    CompletionObservation, HtAmpduTxConfig, HtChannelWidth, HtGuardInterval, HtMcs, HtRate,
    LegacyTxQueue,
    ampdu::{HtAmpduTxResources, HtAmpduTxStorage, RetainedAmpduDmaStorage, RetainedDmaAmpduTx},
};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer};

const HELLO: u32 = 40;
const QUIET_MS: u32 = 1_500;
/// The largest aggregate.
const SUBFRAMES: usize = 8;
/// How long an aggregate's completion is awaited.
const COMPLETION_MS: u32 = 100;
const WORDS: usize = (CELL_F_FRAMES as usize).div_ceil(32);
const AMSDU_WORDS: usize = (CELL_F_AMSDU_FRAMES as usize).div_ceil(32);
/// The A-MSDU stage's cases, in the order of their aggregates.
const AMSDU_CASES: [Case; 3] = [Case::Amsdu1, Case::Amsdu2, Case::AmsduLong];

oer_chips_esp32s31_hal::dma_static! {
    static CHAIN: AmpduDmaStorage<SUBFRAMES, 0> = zeroed in ".dma.bss.mac_reset_ampdu_chain";
}
static METADATA: static_cell::StaticCell<HtAmpduTxStorage<SUBFRAMES>> =
    static_cell::StaticCell::new();
static RETENTION: static_cell::StaticCell<RetainedAmpduDmaStorage<Lease, SUBFRAMES>> =
    static_cell::StaticCell::new();

/// One aggregate size's ends: aggregates, completions with status 0, with a
/// usable Block Ack, other statuses, completions not seen in time.
type Ends = [u32; 5];

pub(super) async fn series(parts: AmsduParts<'_>, access: &mut RadioAccess) -> Result<(), Failure> {
    let AmsduParts {
        frames,
        ordinary,
        station,
        peer,
        journal,
    } = parts;
    mark(100);
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(QUIET_MS))
    {
        EmbassyClock.wait_until(at).await;
    }
    for hello in 0..HELLO {
        let case = Case::Provenance700;
        let counter = case.counter(hello);
        let lease = frames.lease_frame(ORDINARY_SLOT, case.length(), |offset| {
            byte(case, station, peer, counter, offset)
        })?;
        start(
            access,
            ordinary,
            lease,
            case.length(),
            frames.power,
            TIMEOUT,
        )?;
        await_completion(access, ordinary, 0).await?;
    }
    // The owner, as `station/src/radio.rs` builds it.
    let chain: Placed<AmpduDmaStorage<SUBFRAMES, 0>> =
        CHAIN.take().ok_or((tx_phase::POOL, 0x40))?;
    let metadata = METADATA
        .try_init(HtAmpduTxStorage::new())
        .ok_or((tx_phase::POOL, 0x41))?;
    let retention = RETENTION
        .try_init(RetainedAmpduDmaStorage::new())
        .ok_or((tx_phase::POOL, 0x42))?;
    let resources =
        HtAmpduTxResources::pin_static(metadata, chain).map_err(|_| (tx_phase::POOL, 0x43))?;
    let mut owner = RetainedDmaAmpduTx::new(resources, retention);
    let rate = HtRate::new(
        HtMcs::Mcs0,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz20,
    );
    mark(101);
    let mut sender = Aggregates {
        owner: &mut owner,
        frames,
        station,
        peer,
        rate,
        sequence: 0,
    };
    // The control (`tasks/173`): QoS Data without A-MSDU, 2 then 8 a PPDU.
    let mut acked = [0_u32; WORDS];
    let mut ends: [Ends; 2] = [[0; 5]; 2];
    let mut index = 0_u32;
    for (size, aggregates) in [(2_usize, CELL_F_PAIRS), (SUBFRAMES, CELL_F_OCTETS)] {
        let row = ends
            .get_mut(usize::from(size == SUBFRAMES))
            .ok_or((tx_phase::COMPLETION, 0x57))?;
        for _ in 0..aggregates {
            sender
                .send(access, Case::Control, index, size, row, &mut acked)
                .await?;
            index = index.saturating_add(size as u32);
        }
    }
    // The A-MSDU stage: each case in its own aggregates, the three cases in
    // turn; the first half of each case's frames 2 a PPDU, the second 8 (the
    // long case 2 throughout).
    let mut amsdu_acked = [[0_u32; AMSDU_WORDS]; AMSDU_CASES.len()];
    let mut amsdu_ends: [[Ends; 2]; AMSDU_CASES.len()] = [[[0; 5]; 2]; AMSDU_CASES.len()];
    let mut next = [0_u32; AMSDU_CASES.len()];
    for half in 1..=2_u32 {
        let limit = CELL_F_AMSDU_FRAMES / 2 * half;
        while next.iter().any(|sent| *sent < limit) {
            for (at, case) in AMSDU_CASES.into_iter().enumerate() {
                let size = if half == 1 || case == Case::AmsduLong {
                    2
                } else {
                    SUBFRAMES
                };
                let (Some(sent), Some(row), Some(acked)) = (
                    next.get_mut(at),
                    amsdu_ends
                        .get_mut(at)
                        .and_then(|rows| rows.get_mut(usize::from(size == SUBFRAMES))),
                    amsdu_acked.get_mut(at),
                ) else {
                    return Err((tx_phase::COMPLETION, 0x5b));
                };
                if *sent >= limit {
                    continue;
                }
                sender.send(access, case, *sent, size, row, acked).await?;
                *sent = sent.saturating_add(size as u32);
            }
        }
    }
    let records =
        core::iter::once((Case::Control, ends)).chain(AMSDU_CASES.into_iter().zip(amsdu_ends));
    for (case, rows) in records {
        for (size, row) in [2_u32, SUBFRAMES as u32].into_iter().zip(rows) {
            journal.record(
                Kind::AmpduSent,
                &[case.tag(), size, row[0], row[1], row[2], row[3], row[4]],
            );
        }
    }
    let bitmaps = core::iter::once((Case::Control, &acked[..])).chain(
        AMSDU_CASES
            .iter()
            .copied()
            .zip(amsdu_acked.iter().map(|words| &words[..])),
    );
    for (case, words) in bitmaps {
        for (chunk, words) in words.chunks(19).enumerate() {
            // The case's tag, the chunk's first counter, its words.
            let mut values = [0_u32; 21];
            let [tag, from, rest @ ..] = &mut values;
            *tag = case.tag();
            *from = (chunk * 19 * 32) as u32;
            for (cell, word) in rest.iter_mut().zip(words) {
                *cell = *word;
            }
            journal.record(Kind::AmpduAcked, &values);
        }
    }
    mark(102);
    Ok(())
}

/// The owner and what every aggregate of the run shares.
struct Aggregates<'a, 'o> {
    owner: &'o mut RetainedDmaAmpduTx<'static, Lease, SUBFRAMES>,
    frames: &'a super::transmit::Frames,
    station: [u8; 6],
    peer: [u8; 6],
    rate: HtRate,
    /// The next subframe's sequence number, counted over the whole run.
    sequence: u16,
}

impl Aggregates<'_, '_> {
    /// One aggregate of `size` subframes of `case`, its counters `first`
    /// onward, awaited: `row` counts its end, `acked` takes each subframe's
    /// Block Ack bit at its counter.
    async fn send<const N: usize>(
        &mut self,
        access: &mut RadioAccess,
        case: Case,
        first: u32,
        size: usize,
        row: &mut Ends,
        acked: &mut [u32; N],
    ) -> Result<(), Failure> {
        let (station, peer) = (self.station, self.peer);
        let cookie = self.owner.begin().map_err(|_| (tx_phase::FRAME, 0x50))?;
        let mut sequences = [0_u16; SUBFRAMES];
        for (slot, kept) in sequences.iter_mut().enumerate().take(size) {
            let counter = case.counter(first + slot as u32);
            *kept = self.sequence;
            let control = (self.sequence << 4).to_le_bytes();
            self.sequence = (self.sequence + 1) % 4096;
            let length = case.length();
            let lease = self.frames.lease_frame(slot as u8, length + 8, |_| 0)?;
            self.owner
                .intake(cookie, lease, |frame| {
                    for (offset, cell) in frame.iter_mut().take(length).enumerate() {
                        *cell = match offset {
                            22 | 23 => control.get(offset - 22).copied().unwrap_or(0),
                            _ => byte(case, station, peer, counter, offset),
                        };
                    }
                    u16::try_from(length).ok()
                })
                .map_err(|_| (tx_phase::FRAME, 0x51 | (slot as u32) << 8))?;
        }
        let aggregate = self
            .owner
            .prepare_format(cookie, |_| Some(0), false)
            .map_err(|_| (tx_phase::FRAME, 0x52))?;
        let mut config = HtAmpduTxConfig::new(self.rate, aggregate.bytes, aggregate.subframes)
            .ok_or((tx_phase::FRAME, 0x53))?;
        config.data_power_primary = self.frames.power;
        config.data_power_alternate = self.frames.power;
        config.timeout = TIMEOUT;
        self.owner
            .submit(access, cookie, LegacyTxQueue::BestEffort, config)
            .map_err(|_| (tx_phase::START, 0x54))?;
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(COMPLETION_MS))
            .ok_or((tx_phase::COMPLETION, 0x55))?;
        let completion = loop {
            match self.owner.acknowledge_completion(access) {
                Ok(Some(CompletionObservation::Observed(completion))) => break Some(completion),
                Ok(_) => {}
                Err(_) => return Err((tx_phase::COMPLETION, 0x56)),
            }
            if EmbassyClock.now() >= deadline {
                break None;
            }
            if let Some(at) = EmbassyClock.now().checked_add(Duration::from_micros(20)) {
                EmbassyClock.wait_until(at).await;
            }
        };
        row[0] = row[0].saturating_add(1);
        let Some(completion) = completion else {
            row[4] = row[4].saturating_add(1);
            return Err((tx_phase::COMPLETION, 0x58 | first << 8));
        };
        match completion.tx.status() {
            0 => row[1] = row[1].saturating_add(1),
            _ => row[3] = row[3].saturating_add(1),
        }
        if let Some(block_ack) = completion.valid_block_ack() {
            row[2] = row[2].saturating_add(1);
            // Each subframe's bit: its sequence number's offset from the
            // Block Ack's starting sequence.
            let start = block_ack.starting_sequence.get();
            for (slot, sequence) in sequences.iter().take(size).enumerate() {
                let offset = sequence.wrapping_sub(start) % 4096;
                let index = first + slot as u32;
                if offset < 64
                    && block_ack.bitmap >> offset & 1 != 0
                    && let Some(word) = acked.get_mut((index / 32) as usize)
                {
                    *word |= 1 << (index % 32);
                }
            }
        }
        self.owner
            .detach_completed(access, cookie)
            .map_err(|_| (tx_phase::RELEASE, 0x59))?;
        self.owner
            .release_completed(cookie, drop)
            .map_err(|_| (tx_phase::RELEASE, 0x5a))?;
        Ok(())
    }
}
