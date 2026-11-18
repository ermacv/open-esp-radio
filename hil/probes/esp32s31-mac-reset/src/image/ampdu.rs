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
use crate::case_c::{CELL_F_FRAMES, CELL_F_OCTETS, CELL_F_PAIRS, Case, byte};
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
    let mut acked = [0_u32; WORDS];
    let mut ends: [Ends; 2] = [[0; 5]; 2];
    let mut index = 0_u32;
    for (size, aggregates) in [(2_usize, CELL_F_PAIRS), (SUBFRAMES, CELL_F_OCTETS)] {
        let at = usize::from(size == SUBFRAMES);
        for _ in 0..aggregates {
            let first = index;
            let cookie = owner.begin().map_err(|_| (tx_phase::FRAME, 0x50))?;
            for slot in 0..size {
                let case = Case::Control;
                let counter = case.counter(index);
                // The subframe's sequence number: its index, modulo 4096.
                let sequence = ((index % 4096) as u16) << 4;
                let length = case.length();
                let lease = frames.lease_frame(slot as u8, length + 8, |_| 0)?;
                owner
                    .intake(cookie, lease, |frame| {
                        for (offset, cell) in frame.iter_mut().take(length).enumerate() {
                            *cell = match offset {
                                22 | 23 => sequence.to_le_bytes()[offset - 22],
                                _ => byte(case, station, peer, counter, offset),
                            };
                        }
                        u16::try_from(length).ok()
                    })
                    .map_err(|_| (tx_phase::FRAME, 0x51 | (slot as u32) << 8))?;
                index = index.saturating_add(1);
            }
            let aggregate = owner
                .prepare_format(cookie, |_| Some(0), false)
                .map_err(|_| (tx_phase::FRAME, 0x52))?;
            let mut config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes)
                .ok_or((tx_phase::FRAME, 0x53))?;
            config.data_power_primary = frames.power;
            config.data_power_alternate = frames.power;
            config.timeout = TIMEOUT;
            owner
                .submit(access, cookie, LegacyTxQueue::BestEffort, config)
                .map_err(|_| (tx_phase::START, 0x54))?;
            let deadline = EmbassyClock
                .now()
                .checked_add(Duration::from_millis(COMPLETION_MS))
                .ok_or((tx_phase::COMPLETION, 0x55))?;
            let completion = loop {
                match owner.acknowledge_completion(access) {
                    Ok(Some(CompletionObservation::Observed(completion))) => {
                        break Some(completion);
                    }
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
            let row = ends.get_mut(at).ok_or((tx_phase::COMPLETION, 0x57))?;
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
                for subframe in first..index {
                    let sequence = (subframe % 4096) as u16;
                    let offset = sequence.wrapping_sub(start) % 4096;
                    if offset < 64 && block_ack.bitmap >> offset & 1 != 0 {
                        if let Some(word) = acked.get_mut((subframe / 32) as usize) {
                            *word |= 1 << (subframe % 32);
                        }
                    }
                }
            }
            owner
                .detach_completed(access, cookie)
                .map_err(|_| (tx_phase::RELEASE, 0x59))?;
            owner
                .release_completed(cookie, drop)
                .map_err(|_| (tx_phase::RELEASE, 0x5a))?;
        }
    }
    for (size, row) in [2_u32, SUBFRAMES as u32].into_iter().zip(ends) {
        journal.record(
            Kind::AmpduSent,
            &[size, row[0], row[1], row[2], row[3], row[4]],
        );
    }
    for (chunk, words) in acked.chunks(20).enumerate() {
        let mut values = [0_u32; 21];
        if let Some(head) = values.first_mut() {
            *head = (chunk * 20 * 32) as u32;
        }
        for (cell, word) in values.iter_mut().skip(1).zip(words) {
            *cell = *word;
        }
        journal.record(Kind::AmpduAcked, &values);
    }
    mark(102);
    Ok(())
}
