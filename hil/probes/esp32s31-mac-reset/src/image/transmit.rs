//! The sender's transmit half: frames of case C (`tasks/154`, `159`) or of
//! cell E (`tasks/161`) on the ordinary queue, each awaited, to the peer
//! the session learned, with the direct-MPDU profile at 6 Mb/s.
use super::*;
use crate::radio::Sources;
use oer_chips_esp32s31_hal::radio::ieee80211::tx_frame::{TxCapacity, TxEntry, TxFrame};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::dma::tx_storage::{PinnedTxDmaStorage, TxDmaAccess, TxDmaStorage},
    system::memory::dma_static::{PlacedTxLease, PlacedTxPool},
    system::owner::RadioAccess,
    types::{
        MacInterface, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
        MacLegacyTxResponse, MacTxControlFrame, MacTxDetachOutcome, MacTxDetachReason,
        MacTxProtection, MacTxQueueIndex,
    },
};
use oer_portable_memory::{FrameLen, PinnedDmaTxPool, PinnedDmaTxRadioLease};

/// Case C's longest frame (`tasks/154`, `159`): the provenance series' 1 500.
pub(super) const FRAME: usize = 1_500;
/// The TX metadata entry before the frame in its buffer
/// (`WIFI_TX_METADATA_ENTRY`): the first word holds the MPDU's length with
/// its FCS, the second is zero.
const METADATA: usize = 8;
/// The FCS the hardware appends, its room left zero after the frame.
const FCS: usize = 4;
/// A pool slot: a frame of [`FRAME`] with its metadata and FCS room.
const SLOT: usize = METADATA + FRAME + FCS;
/// The ordinary frames' slot and one spare.
const SLOTS: usize = 2;
/// The ordinary frames' slot.
pub(super) const ORDINARY_SLOT: u8 = (SLOTS - 1) as u8;
/// The queue's timeout of the backend's direct-MPDU profile.
pub(super) const TIMEOUT: u16 = 0x03ff;
/// How long a frame's completion is awaited.
const COMPLETION_MS: u32 = 20;
/// The transmit queue every frame goes on.
const QUEUE: MacTxQueueIndex = MacTxQueueIndex::Zero;

type Pool = PinnedDmaTxPool<SLOT, 0, 0, SLOTS>;
type Placed = PlacedTxPool<SLOT, 0, 0, SLOTS>;
pub(super) type Lease = PlacedTxLease<PinnedDmaTxRadioLease<'static, SLOT, 0, 0>>;
pub(super) type Descriptor = PinnedTxDmaStorage<Lease>;

/// Where the pinned pool lives for the run.
static POOL_PLACE: static_cell::StaticCell<Placed> = static_cell::StaticCell::new();
oer_chips_esp32s31_hal::dma_static! {
    static TX_POOL: Pool = zeroed in ".dma.bss.case_c_sender_tx_pool";
    static TX_ORDINARY: TxDmaStorage = zeroed in ".dma.bss.case_c_sender_tx_ordinary";
}

/// The phases the transmit half's failures name.
pub(super) mod tx_phase {
    pub const POOL: u32 = 30;
    pub const PEER: u32 = 31;
    pub const FRAME: u32 = 32;
    pub const START: u32 = 33;
    pub const COMPLETION: u32 = 34;
    pub const RELEASE: u32 = 35;
}

/// The bytes a frame of `length` octets takes in its buffer.
const fn transfer(length: usize) -> usize {
    METADATA + length + FCS
}

/// The legacy program of one frame: the backend's direct-MPDU profile
/// (`LegacyTxConfig::management`) at 6 Mb/s, answered by an ACK.
fn parameters(length: usize, power: u8, timeout: u16) -> MacLegacyTxParameters {
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
        data_power: power,
        timeout,
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

/// The pool, held for the whole run.
pub(super) struct Frames {
    pool: &'static Placed,
    /// The data power code at 6 Mb/s: the calibrated profile's, under the
    /// regulatory ceiling.
    pub(super) power: u8,
}

impl Frames {
    /// Slot `slot`'s frame of `length` octets, each `byte(offset)`, behind
    /// its TX metadata, as the radio holds it.
    pub(super) fn lease_frame(
        &self,
        slot: u8,
        length: usize,
        byte: impl Fn(usize) -> u8,
    ) -> Result<Lease, Failure> {
        let frame = length;
        let head = ((frame + FCS) as u32).to_le_bytes();
        let length = FrameLen::new(transfer(frame)).map_err(|_| (tx_phase::FRAME, 0))?;
        let network = self
            .pool
            .claim_network(slot)
            .map_err(|_| (tx_phase::FRAME, 1 | u32::from(slot) << 8))?;
        let _published = network.publish(length, |bytes| {
            for (offset, cell) in bytes.iter_mut().enumerate() {
                *cell = match (head.get(offset), offset.checked_sub(METADATA)) {
                    (Some(byte), _) => *byte,
                    (None, Some(at)) if at < frame => byte(at),
                    _ => 0,
                };
            }
        });
        self.pool
            .claim_radio(slot)
            .map_err(|_| (tx_phase::FRAME, 2 | u32::from(slot) << 8))
    }
}

/// A descriptor with its frame attached, reserved and its queue started.
pub(super) fn start(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    lease: Lease,
    length: usize,
    power: u8,
    timeout: u16,
) -> Result<(), Failure> {
    start_keyed(access, descriptor, lease, length, power, timeout, 0)
}

/// [`start`] under hardware key `key`: the MAC seals the frame with that
/// entry's key and appends its MIC, for which `length` already holds room.
pub(super) fn start_keyed(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    lease: Lease,
    length: usize,
    power: u8,
    timeout: u16,
    key: u8,
) -> Result<(), Failure> {
    descriptor.attach(lease).map_err(|_| (tx_phase::START, 1))?;
    let TxDmaAccess::Free(mut free) = descriptor.access() else {
        return Err((tx_phase::START, 2));
    };
    // The HAL's layout: the entry, the frame, its FCS room; the descriptor's
    // capacity the transfer rounded up to a word.
    let frame = TxFrame::lay_out(
        free.buffer_mut().map_err(|_| (tx_phase::START, 3))?,
        length,
        0,
        TxEntry::default(),
        TxCapacity::Minimal,
    )
    .map_err(|_| (tx_phase::START, 3))?;
    free.reserve(frame).map_err(|_| (tx_phase::START, 3))?;
    let publication = descriptor.publication().map_err(|_| (tx_phase::START, 4))?;
    let mut parameters = parameters(length, power, timeout);
    parameters.hardware_key_selector = key;
    let program = MacLegacyTxProgram::new(&publication, parameters).ok_or((tx_phase::START, 5))?;
    let mut hal = access.wifi_mac_hal();
    hal.prepare_bound_legacy_tx(&publication, QUEUE, program)
        .map_err(|_| (tx_phase::START, 6))?;
    hal.start_bound_tx(publication, QUEUE, || {})
        .map_err(|_| (tx_phase::START, 7))?;
    Ok(())
}

/// How one ordinary frame ended: its completion's status.
#[derive(Clone, Copy)]
pub(super) struct Ended {
    pub(super) status: u32,
}

/// Await the queue's completion for at most [`COMPLETION_MS`]; the
/// descriptor is free again and its lease back in the pool.
pub(super) async fn await_completion(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
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
            return Err((tx_phase::COMPLETION, 1));
        }
        wait_micros(50).await;
    };
    complete(access, descriptor, observation)
}

/// A completion taken: the descriptor released by the queue's detach.
fn complete(
    access: &mut RadioAccess,
    descriptor: &mut Descriptor,
    observation: oer_chips_esp32s31_hal::types::MacTxCompletionObservation,
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
        status: u32::from(observation.status()),
    })
}

/// The pool and the ordinary descriptor, then case C's rounds or cell E's.
pub(super) async fn series(
    access: &mut RadioAccess,
    sources: &Sources,
    station: [u8; 6],
    peer: [u8; 6],
    journal: &mut Journal,
) -> Result<(), Failure> {
    let pool =
        Placed::pin(TX_POOL.take().ok_or((tx_phase::POOL, 1))?).map_err(|_| (tx_phase::POOL, 2))?;
    let pool: &'static Placed = POOL_PLACE.try_init(pool).ok_or((tx_phase::POOL, 5))?;
    let mut ordinary: Descriptor =
        oer_chips_esp32s31_hal::radio::ieee80211::dma::tx_storage::TxDmaStorage::pin::<
            oer_chips_esp32s31_hal::radio::ieee80211::dma::space::ChipDma,
            Lease,
        >(TX_ORDINARY.take().ok_or((tx_phase::POOL, 3))?)
        .map_err(|_| (tx_phase::POOL, 4))?;
    let frames = Frames {
        pool,
        power: sources.tx_power.pair(MacLegacyRate::Ofdm6M.code()).primary as u8,
    };
    super::amsdu::series(
        super::amsdu::AmsduParts {
            frames: &frames,
            ordinary: &mut ordinary,
            station,
            peer,
            journal,
        },
        access,
    )
    .await
}
