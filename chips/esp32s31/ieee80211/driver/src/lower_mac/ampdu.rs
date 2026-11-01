//! The A-MPDU hardware sequence of the lower-MAC core.
//!
//! One aggregate is one attempt: the core plans it
//! ([`LowerMacCore::plan_ampdu`](super::LowerMacCore::plan_ampdu)),
//! [`publish`] has [`RetainedDmaAmpduTx`] begin it, commit every subframe
//! with `commit_ht` or `commit_he` and publish it once, and
//! [`LowerMacCore::service_ampdu`](super::LowerMacCore::service_ampdu) ends
//! it. Its completion reports the recipient's BlockAck; the aggregate is
//! never retained for a retry (`retain_for_ampdu_retry`), since selecting
//! what a later aggregate retransmits is the portable layer's.
//!
//! # Buffers
//!
//! An [`Esp32s31AmpduBuffer`] is one idle aggregate owner together with the
//! subframes the caller appended. Each subframe is a [`PlacedBacking`]
//! lease of the transmit pool every MPDU is written into: the MPDU follows
//! the eight-byte metadata prefix at the start of the lease's region, and
//! `commit_ht` publishes it where it was written. The owner retains every
//! lease until the hardware left the chain; its release hands each lease
//! back to the caller in subframe order, so a later aggregate can send the
//! same MPDU again.
//!
//! # Limits
//!
//! HT and HE SU, at most `SLOTS` subframes and, at each rate, the octets
//! [`esp32s31_ampdu_length`] gives ([`ESP32S31_AMPDU_LENGTHS`] is the same
//! limit as the port's table). The HE path carries the recipient's density
//! and the access category's TXOP limit in its checked plan. Trigger-based
//! aggregates stay outside the declared formats; they require a separate
//! Trigger reservation.

use super::{FreeQueue, TxAdmission};
use core::fmt;
use oer_radio_port::{PoolPath, Root};

use crate::mac::tx::{
    HeAmpduTxConfig, HtAmpduDensity, HtChannelWidth, HtGuardInterval, HtMcs, HtProtectionSpacing,
    HtRate, LegacyTxQueue, TxControlFrame, TxCookie, TxDmaState,
    ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
        HtAmpduHardware, HtAmpduTxCompletion, HtAmpduTxError, RetainedDmaAmpduTx,
        TX_AMPDU_METADATA_SIZE,
    },
};
use oer_esp32s31_hal::memory::dma_static::PlacedBacking;
use oer_ieee80211_contracts_port::{
    AmpduCapabilities, AmpduLengthTable, BlockAckReport, PhyFormatSet, TxStatus,
};

use crate::{
    ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config},
    ordinary_tx::{TX_ABORT_SETTLE, TX_CCMP_MIC_SIZE, TX_FCS_SIZE},
    tx::WifiTxWake,
};

/// The longest aggregate the core publishes at any rate: the 16 bits of the
/// HT-SIG HT Length field (IEEE Std 802.11-2020, 19.3.9.4.3). Every rate's
/// length ([`esp32s31_ampdu_length`]) lies within it, and the aggregate
/// owner is configured to it; [`AmpduBudget`] holds each aggregate to its
/// rate's length.
pub const ESP32S31_AMPDU_MAX_LENGTH: u16 = u16::MAX;

/// The longest HE aggregate: the vendor's lowest HT ceiling (MCS 0, long
/// guard interval, [`HtRate::vendor_ampdu_byte_limit`]) at every HE20 SU
/// rate. Which rate code the vendor's per-rate selector receives for an HE
/// rate is not established, so no HE rate takes a higher HT ceiling.
pub const ESP32S31_HE_AMPDU_MAX_LENGTH: u16 = match HtRate::new(
    HtMcs::Mcs0,
    HtGuardInterval::Long800Ns,
    HtChannelWidth::Mhz20,
)
.vendor_ampdu_byte_limit()
{
    Some(limit) => limit,
    None => panic!("the vendor sets a ceiling at HT MCS 0"),
};

/// The longest aggregate the core publishes at `rate`: at an HT rate the
/// vendor's per-rate ceiling (`rx11NRate2AMPDULimit`, see
/// [`HtRate::vendor_ampdu_byte_limit`]), or [`ESP32S31_AMPDU_MAX_LENGTH`]
/// where the vendor sets none; at an HE rate
/// [`ESP32S31_HE_AMPDU_MAX_LENGTH`]; `None` at a legacy rate.
pub const fn esp32s31_ampdu_length(rate: crate::mac::tx::TxPhyRate) -> Option<u16> {
    use crate::mac::tx::TxPhyRate;
    match rate {
        TxPhyRate::Ht(rate) => Some(match rate.vendor_ampdu_byte_limit() {
            Some(limit) => limit,
            None => ESP32S31_AMPDU_MAX_LENGTH,
        }),
        TxPhyRate::He(_) => Some(ESP32S31_HE_AMPDU_MAX_LENGTH),
        TxPhyRate::Legacy(_) => None,
    }
}

/// [`esp32s31_ampdu_length`] at every portable rate the transmit formatter
/// sends (HT MCS 0 to 7 at 20 and 40 MHz with either guard interval, HE20
/// SU MCS 0 to 9 with every guard interval); zero at every other rate.
pub static ESP32S31_AMPDU_LENGTHS: AmpduLengthTable = esp32s31_ampdu_lengths();

const fn esp32s31_ampdu_lengths() -> AmpduLengthTable {
    use crate::mac::tx::TxPhyRate;
    // The vendor's selector takes the rate code alone: both widths share
    // one length.
    const fn ht(mcs: u8, guard: HtGuardInterval) -> u32 {
        let Some(mcs) = HtMcs::from_index(mcs) else {
            panic!("HT MCS 0 to 7");
        };
        match esp32s31_ampdu_length(TxPhyRate::Ht(HtRate::new(
            mcs,
            guard,
            HtChannelWidth::Mhz20,
        ))) {
            Some(length) => length as u32,
            None => 0,
        }
    }
    let mut table = AmpduLengthTable::uniform(0);
    let mut mcs = 0;
    while mcs <= HtMcs::Mcs7.index() {
        let long = ht(mcs, HtGuardInterval::Long800Ns);
        let short = ht(mcs, HtGuardInterval::Short400Ns);
        let index = mcs as usize;
        table.ht[0][0][index] = long;
        table.ht[1][0][index] = long;
        table.ht[0][1][index] = short;
        table.ht[1][1][index] = short;
        mcs += 1;
    }
    let mut mcs = 0;
    while mcs < 10 {
        let mut guard = 0;
        while guard < 3 {
            table.he[0][guard][mcs] = ESP32S31_HE_AMPDU_MAX_LENGTH as u32;
            guard += 1;
        }
        mcs += 1;
    }
    table
}

/// The limits of the core's aggregates of up to `slots` subframes.
pub const fn esp32s31_ampdu_capabilities(slots: usize) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: if slots > u16::MAX as usize {
            u16::MAX
        } else {
            slots as u16
        },
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: &ESP32S31_AMPDU_LENGTHS,
    }
}

/// The length rule of one aggregate: what [`super::LowerMacCore::plan_ampdu`]
/// checks, from the rate, the access category's advertised TXOP limit an HE
/// aggregate carries and the recipient's density, and what a port checks
/// subframe by subframe before it accepts one into an aggregate.
///
/// The empty delimiters an HE subframe needs grow with the density, so a
/// budget at the widest density (16 µs) admits nothing the recipient's own
/// density refuses.
pub struct AmpduBudget {
    budget: crate::mac::tx::ampdu::HtAmpduLengthAccumulator,
    he: Option<HeAmpduPolicy>,
}

impl AmpduBudget {
    /// The widest MPDU start spacing (16 µs, IEEE encoding 7).
    pub const WIDEST_DENSITY: HtAmpduDensity = HtAmpduDensity::from_spacing_code(7);

    /// An empty budget of up to `slots` subframes at `rate`.
    ///
    /// # Errors
    ///
    /// A non-HT rate, or an HE TXOP limit with no room for any subframe.
    pub fn new(
        rate: crate::mac::tx::TxPhyRate,
        txop_limit_units_32_us: u16,
        density: HtAmpduDensity,
        slots: usize,
    ) -> Result<Self, super::AmpduRefusal> {
        use crate::mac::tx::{HeEdcaTxopLimit, TxPhyRate, ampdu::HtAmpduLengthAccumulator};
        let max_subframes = u8::try_from(slots.min(32)).unwrap_or(32);
        let (max_bytes, he) = match rate {
            TxPhyRate::Ht(_) => (
                esp32s31_ampdu_length(rate).ok_or(super::AmpduRefusal::LegacyRate)?,
                None,
            ),
            TxPhyRate::He(rate) => {
                let txop = crate::mac::tx::runtime::he_txop_limit(
                    txop_limit_units_32_us,
                    HeEdcaTxopLimit::DEFAULT,
                )
                .map_err(|_| super::AmpduRefusal::Length)?;
                let policy = HeAmpduPolicy::new(rate, density, txop);
                let apep = rate
                    .checked_maximum_apep_bytes(txop)
                    .ok_or(super::AmpduRefusal::Length)?;
                let max_bytes = u16::try_from(apep)
                    .unwrap_or(u16::MAX)
                    .min(ESP32S31_HE_AMPDU_MAX_LENGTH);
                (max_bytes, Some(policy))
            }
            TxPhyRate::Legacy(_) => return Err(super::AmpduRefusal::LegacyRate),
        };
        let budget = HtAmpduLengthAccumulator::new(max_subframes, max_bytes)
            .map_err(|_| super::AmpduRefusal::Length)?;
        Ok(Self { budget, he })
    }

    /// The HE policy the budget follows; `None` for HT.
    pub const fn he_policy(&self) -> Option<HeAmpduPolicy> {
        self.he
    }

    /// Count one more subframe of `psdu` octets: its MPDU with the hardware
    /// MIC and FCS.
    ///
    /// # Errors
    ///
    /// The aggregate would grow past the format's length or subframe limit;
    /// the budget is unchanged.
    pub fn push(&mut self, psdu: usize) -> Result<(), super::AmpduRefusal> {
        let delimiters = match self.he {
            None => 0,
            Some(policy) => {
                let psdu = u16::try_from(psdu).map_err(|_| super::AmpduRefusal::Length)?;
                policy
                    .rate()
                    .ampdu_empty_delimiters(psdu, policy.density())
                    .ok_or(super::AmpduRefusal::Length)?
            }
        };
        let psdu = u32::try_from(psdu).map_err(|_| super::AmpduRefusal::Length)?;
        self.budget
            .push(psdu, delimiters)
            .map_err(|_| super::AmpduRefusal::Length)
    }
}

/// The aggregate owner behind one buffer. The port publishes only
/// referenced subframes, so its formatter workspace is empty.
pub type Esp32s31AmpduOwner<'slot, B, const SLOTS: usize> = RetainedDmaAmpduTx<'slot, B, SLOTS>;

struct Subframe<B> {
    backing: B,
    len: usize,
}

/// One aggregate being built: an idle aggregate owner and the subframes
/// appended so far, each in a buffer of its own. [`publish`] takes it;
/// [`Self::into_owner`] gives the owner and the buffers back unpublished.
pub struct Esp32s31AmpduBuffer<'slot, L, const SLOTS: usize> {
    pub(super) owner: Esp32s31AmpduOwner<'slot, L, SLOTS>,
    subframes: [Option<Subframe<L>>; SLOTS],
    // Separate storage avoids per-entry alignment padding on RV32.
    admitted_at: [oer_time::Instant; SLOTS],
    count: usize,
}

impl<'slot, L, const SLOTS: usize> Esp32s31AmpduBuffer<'slot, L, SLOTS> {
    /// An empty aggregate over the idle `owner`, the owner configured to
    /// [`ESP32S31_AMPDU_MAX_LENGTH`]; the rate's length is the plan's
    /// ([`AmpduBudget`]).
    ///
    /// # Errors
    ///
    /// The owner, when it is not idle.
    #[allow(clippy::result_large_err, reason = "the owner goes back by value")]
    pub fn new(
        mut owner: Esp32s31AmpduOwner<'slot, L, SLOTS>,
    ) -> Result<Self, Esp32s31AmpduOwner<'slot, L, SLOTS>> {
        if owner.state() != TxDmaState::Free
            || owner
                .configure_max_aggregate_bytes(ESP32S31_AMPDU_MAX_LENGTH)
                .is_err()
        {
            return Err(owner);
        }
        Ok(Self {
            owner,
            subframes: [const { None }; SLOTS],
            admitted_at: [oer_time::Instant::EPOCH; SLOTS],
            count: 0,
        })
    }

    /// The appended MPDU lengths, in order.
    pub fn lengths(&self) -> impl Iterator<Item = usize> + '_ {
        self.subframes.iter().flatten().map(|subframe| subframe.len)
    }

    /// Hand every subframe's buffer to `returned`, in subframe order, and
    /// give the owner back.
    pub fn into_owner(self, mut returned: impl FnMut(L)) -> Esp32s31AmpduOwner<'slot, L, SLOTS> {
        let Self {
            owner, subframes, ..
        } = self;
        for subframe in subframes.into_iter().flatten() {
            returned(subframe.backing);
        }
        owner
    }
}

impl<L: PlacedBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'_, L, SLOTS> {
    pub(super) fn matches_admission<Path: PoolPath>(
        &mut self,
        role: oer_ieee80211_contracts_port::VifRole,
        admission: &TxAdmission<'_, Path>,
    ) -> bool {
        self.subframes.iter_mut().flatten().all(|subframe| {
            let region = subframe.backing.stable_dma_region().into_mut_slice();
            admission.matches_frame(
                role,
                region
                    .get(TX_AMPDU_METADATA_SIZE..)
                    .and_then(|frame| frame.get(..subframe.len)),
            )
        })
    }

    /// The first MPDU, for the checks of admission.
    pub fn first_mpdu(&mut self) -> Option<&[u8]> {
        let subframe = self.subframes.first_mut()?.as_mut()?;
        let len = subframe.len;
        let region = subframe.backing.stable_dma_region().into_mut_slice();
        Some(&region[TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + len])
    }
}

/// The region one subframe of `len` MPDU bytes needs: the metadata prefix,
/// the MPDU, room for the hardware CCMP MIC and FCS, word-aligned.
const fn subframe_capacity(len: usize) -> Option<usize> {
    match (TX_AMPDU_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE + 3).checked_add(len) {
        Some(capacity) => Some(capacity & !3),
        None => None,
    }
}

impl<L: PlacedBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'_, L, SLOTS> {
    /// Append the subframe `backing` holds, an MPDU of `len` octets after
    /// the metadata prefix, admitted at `admitted_at`: its MPDU memory.
    ///
    /// # Errors
    ///
    /// The backing, unchanged, when the aggregate holds `SLOTS`, the length
    /// is zero, or the backing is too short for the MPDU with its metadata,
    /// MIC and FCS.
    pub fn push_backing(
        &mut self,
        mut backing: L,
        len: usize,
        admitted_at: oer_time::Instant,
    ) -> Result<&mut [u8], L> {
        let fits = subframe_capacity(len)
            .is_some_and(|capacity| backing.stable_dma_region().len() >= capacity);
        if self.count >= SLOTS || len == 0 || !fits {
            return Err(backing);
        }
        let (Some(at), Some(slot)) = (
            self.admitted_at.get_mut(self.count),
            self.subframes.get_mut(self.count),
        ) else {
            return Err(backing);
        };
        *at = admitted_at;
        let subframe = slot.insert(Subframe { backing, len });
        self.count += 1;
        let region = subframe.backing.stable_dma_region().into_mut_slice();
        Ok(region
            .get_mut(TX_AMPDU_METADATA_SIZE..)
            .and_then(|mpdu| mpdu.get_mut(..len))
            .unwrap_or_default())
    }
}

impl<L, const SLOTS: usize> Esp32s31AmpduBuffer<'_, L, SLOTS> {
    /// The original admission of a retained unpublished subframe.
    pub fn admitted_at(&self, index: usize) -> Option<oer_time::Instant> {
        self.subframes.get(index)?.as_ref()?;
        self.admitted_at.get(index).copied()
    }

    /// Keep exactly the selected original positions after the core has
    /// retired expired members. Their backings and admission ages move
    /// together during compaction. Invalid masks change nothing.
    #[expect(
        clippy::indexing_slicing,
        reason = "push bounds count by SLOTS; compaction's target never exceeds its source index"
    )]
    pub fn retain_subframes(&mut self, mask: u64) -> Result<(), super::AmpduRefusal> {
        if self.count > 64 || (self.count < 64 && mask >> self.count != 0) {
            return Err(super::AmpduRefusal::TooManySubframes);
        }
        let mut target = 0;
        for index in 0..self.count {
            let entry = self.subframes[index].take();
            if mask & (1_u64 << index) != 0 {
                self.subframes[target] = entry;
                self.admitted_at[target] = self.admitted_at[index];
                target += 1;
            }
        }
        self.count = target;
        Ok(())
    }

    fn expired_mask(&self, now: oer_time::Instant, lifetime: oer_time::Duration) -> u64 {
        self.subframes
            .iter()
            .zip(self.admitted_at.iter())
            .enumerate()
            .fold(0_u64, |mask, (index, (entry, admitted_at))| {
                if index < 64
                    && entry.as_ref().is_some_and(|_| {
                        now.checked_duration_since(*admitted_at)
                            .is_some_and(|age| age >= lifetime)
                    })
                {
                    mask | (1_u64 << index)
                } else {
                    mask
                }
            })
    }

    /// The subframes appended.
    pub const fn subframes(&self) -> usize {
        self.count
    }
}

impl<L, const SLOTS: usize> fmt::Debug for Esp32s31AmpduBuffer<'_, L, SLOTS> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Esp32s31AmpduBuffer")
            .field("subframes", &self.count)
            .finish_non_exhaustive()
    }
}

/// The affine publication plan owns both its queue and encryption authority.
/// A protected plan cannot be forged or retained after publication.
#[must_use = "publish the aggregate or return its admission"]
pub struct AmpduPlan<'core, Path: PoolPath = Root<oer_radio_port::LocalId<'core>>> {
    pub(super) queue: FreeQueue<'core, Path>,
    pub(super) admission: TxAdmission<'core, Path>,
    pub(super) settings: AmpduSettings,
}

/// Register values already derived from the owned admission.
pub(super) struct AmpduSettings {
    pub lifetime: oer_time::Duration,
    pub queue: LegacyTxQueue,
    pub format: AmpduFormat,
    pub role: AmpduTxRoleAdapter,
    pub hardware_mic_length: u8,
    /// Everything but the aggregate's length and subframe count, which the
    /// owner computes as the subframes are committed.
    pub publication: AmpduPublication,
}

/// The PPDU format of an aggregate and what it alone needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduFormat {
    Ht {
        rate: HtRate,
        protection_spacing: HtProtectionSpacing,
    },
    /// An HE SU aggregate: its rate, recipient density and TXOP limit, and
    /// the BSS color every HE PPDU of the station carries.
    He {
        policy: HeAmpduPolicy,
        bss_color: u8,
    },
}

/// What an aggregate's publication shares in both formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduPublication {
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub control: TxControlFrame,
    pub aifsn: u8,
    pub contention_window: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

/// The owner's protection spacing for a recipient's Minimum MPDU Start
/// Spacing encoding (IEEE 0-7, bits 4:2 of its A-MPDU Parameters).
pub const fn protection_spacing(min_mpdu_start_spacing: u8) -> HtProtectionSpacing {
    HtProtectionSpacing::from_density(HtAmpduDensity::from_spacing_code(min_mpdu_start_spacing))
}

/// A published aggregate on its queue.
pub struct PublishedAmpdu<
    'core,
    'slot,
    B,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    queue: FreeQueue<'core, Path>,
    admission: TxAdmission<'core, Path>,
    owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    cookie: TxCookie,
    /// The publication deadline, or the end of the abort settle.
    deadline: oer_time::Instant,
    settling: bool,
    admitted_at: [oer_time::Instant; SLOTS],
    /// How it ended, once a service edge saw the hardware leave the chain,
    /// or the fault that stopped its service.
    end: Option<Result<AmpduOutcome, HtAmpduTxError>>,
}

/// What one service edge left of a published aggregate served in place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduServed {
    /// It is still the hardware's, or settles its abort.
    Pending,
    /// It ended and released its subframes:
    /// [`PublishedAmpdu::into_progress`] detaches it.
    Ended,
}

impl<'core, 'slot, B, const SLOTS: usize, Path: PoolPath>
    PublishedAmpdu<'core, 'slot, B, SLOTS, Path>
{
    /// The publication after its service: detached once it ended, retained
    /// in quarantine once its service failed, and itself while it runs.
    ///
    /// # Errors
    ///
    /// Its service failed: the fault retains every owner.
    #[allow(
        clippy::result_large_err,
        reason = "a fault retains the complete publication in quarantine"
    )]
    pub fn into_progress(
        self,
    ) -> Result<AmpduProgress<'core, 'slot, B, SLOTS, Path>, AmpduFault<'core, 'slot, B, SLOTS, Path>>
    {
        match self.end {
            None => Ok(AmpduProgress::Pending(self)),
            Some(Ok(outcome)) => Ok(AmpduProgress::Complete(CompletedAmpdu {
                outcome,
                owner: self.owner,
                queue: self.queue,
                admission: self.admission,
                admitted_at: self.admitted_at,
            })),
            Some(Err(error)) => Err(AmpduFault {
                error,
                _owner: self.owner,
                _queue: self.queue,
                _admission: self.admission,
                _admitted_at: self.admitted_at,
            }),
        }
    }
}

impl<B, const SLOTS: usize, Path: PoolPath> PublishedAmpdu<'_, '_, B, SLOTS, Path> {
    pub const fn deadline(&self) -> oer_time::Instant {
        self.deadline
    }

    pub const fn abort_settling(&self) -> bool {
        self.settling
    }

    /// The aggregate owner, for observation.
    #[cfg(test)]
    pub(super) const fn owner(&self) -> &Esp32s31AmpduOwner<'_, B, SLOTS> {
        &self.owner
    }

    /// The hardware index of the aggregate's queue.
    pub(super) fn queue_index(&self) -> usize {
        usize::from(LegacyTxQueue::from_access_category(self.queue.category()).hardware_index())
    }
}

/// How a published aggregate ended.
#[derive(Clone, Copy)]
pub enum AmpduOutcome {
    Completed(HtAmpduTxCompletion),
    Collision,
    /// The queue's hardware timeout and its abort.
    Aborted,
}

impl AmpduOutcome {
    /// How the aggregate ended. A valid BlockAck is the solicited
    /// response, so the attempt succeeded whatever the completion status
    /// says ([`HtAmpduTxCompletion::valid_block_ack`]); a completion without
    /// one is its failure, and a success status without one is `AckTimeout`.
    pub fn end(self) -> super::PublicationEnd {
        match self {
            Self::Completed(completion) => {
                let block_ack = completion.valid_block_ack();
                let status = match (block_ack, completion.tx.tx_status()) {
                    (Some(_), _) => TxStatus::Success,
                    (None, TxStatus::Success) => TxStatus::AckTimeout,
                    (None, failure) => failure,
                };
                super::PublicationEnd::Completed {
                    status,
                    ack_snr_db: completion.tx.ack_snr_sample(),
                    block_ack: block_ack.map(|block_ack| BlockAckReport {
                        start_sequence: block_ack.starting_sequence,
                        bitmap: block_ack.bitmap,
                    }),
                }
            }
            Self::Collision => super::PublicationEnd::Collision,
            Self::Aborted => super::PublicationEnd::TimedOut,
        }
    }
}

pub enum AmpduProgress<
    'core,
    'slot,
    B,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    Pending(PublishedAmpdu<'core, 'slot, B, SLOTS, Path>),
    /// The aggregate ended and released its subframes; the owner is idle.
    Complete(CompletedAmpdu<'core, 'slot, B, SLOTS, Path>),
}

/// The detached aggregate and its affine terminal. Only the installing core
/// releases the queue and key right while publishing the terminal to its sink.
#[must_use = "return the detached aggregate through its core"]
pub struct CompletedAmpdu<
    'core,
    'slot,
    B,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    pub(super) outcome: AmpduOutcome,
    pub(super) owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    pub(super) queue: FreeQueue<'core, Path>,
    pub(super) admission: TxAdmission<'core, Path>,
    pub(super) admitted_at: [oer_time::Instant; SLOTS],
}

/// An aggregate fault retains every admitted right and its DMA owner. It has
/// no release operation: a poisoned backend carries it through reset custody.
#[must_use = "retain the poisoned aggregate in the backend's quarantine"]
pub struct AmpduFault<
    'core,
    'slot,
    B,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    pub error: HtAmpduTxError,
    _owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    _queue: FreeQueue<'core, Path>,
    _admission: TxAdmission<'core, Path>,
    _admitted_at: [oer_time::Instant; SLOTS],
}

impl<'core, 'slot, B, const SLOTS: usize, Path: PoolPath> AmpduFault<'core, 'slot, B, SLOTS, Path> {
    pub(super) fn retain_completed(
        completed: CompletedAmpdu<'core, 'slot, B, SLOTS, Path>,
    ) -> Self {
        Self {
            error: HtAmpduTxError::ResetRequired,
            _owner: completed.owner,
            _queue: completed.queue,
            _admission: completed.admission,
            _admitted_at: completed.admitted_at,
        }
    }
}

impl<B, const SLOTS: usize, Path: PoolPath> fmt::Debug for AmpduFault<'_, '_, B, SLOTS, Path> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AmpduFault")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// A publication refused for age, retaining every original owner.
#[must_use = "complete the expiry terminal and retain the live subframes"]
pub struct ExpiredAmpdu<
    'core,
    'slot,
    L,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    buffer: Esp32s31AmpduBuffer<'slot, L, SLOTS>,
    plan: AmpduPlan<'core, Path>,
    msdus: u64,
}
impl<L, const SLOTS: usize, Path: PoolPath> fmt::Debug for ExpiredAmpdu<'_, '_, L, SLOTS, Path> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExpiredAmpdu")
            .field("msdus", &self.msdus)
            .finish_non_exhaustive()
    }
}
impl<'core, 'slot, L, const SLOTS: usize, Path: PoolPath>
    ExpiredAmpdu<'core, 'slot, L, SLOTS, Path>
{
    /// Original positions of expired MSDUs; the rest remains live.
    pub const fn expired_msdus(&self) -> u64 {
        self.msdus
    }
    pub(super) fn into_parts(
        self,
    ) -> (
        Esp32s31AmpduBuffer<'slot, L, SLOTS>,
        AmpduPlan<'core, Path>,
        u64,
    ) {
        (self.buffer, self.plan, self.msdus)
    }
}
/// Age returns all unpublished owners; a hardware fault quarantines them.
pub enum AmpduStartError<
    'core,
    'slot,
    L,
    const SLOTS: usize,
    Path: PoolPath = Root<oer_radio_port::LocalId<'core>>,
> {
    Expired(ExpiredAmpdu<'core, 'slot, L, SLOTS, Path>),
    Fault(AmpduFault<'core, 'slot, L, SLOTS, Path>),
}
impl<L, const SLOTS: usize, Path: PoolPath> fmt::Debug for AmpduStartError<'_, '_, L, SLOTS, Path> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expired(expired) => f.debug_tuple("Expired").field(&expired.msdus).finish(),
            Self::Fault(fault) => fault.fmt(f),
        }
    }
}

/// Commit the buffer's subframes to its owner and publish the aggregate
/// once. Expiry returns every unpublished owner; a hardware fault keeps
/// the owner and its admission in sealed quarantine.
#[allow(
    clippy::result_large_err,
    reason = "retain the complete DMA and key owners without allocation"
)]
pub fn publish<'core, 'slot, L, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    buffer: Esp32s31AmpduBuffer<'slot, L, SLOTS>,
    plan: AmpduPlan<'core, Path>,
    now: oer_time::Instant,
    publication_timeout: oer_time::Duration,
) -> Result<
    PublishedAmpdu<'core, 'slot, L, SLOTS, Path>,
    AmpduStartError<'core, 'slot, L, SLOTS, Path>,
>
where
    L: PlacedBacking,
    H: HtAmpduHardware,
{
    let expired = buffer.expired_mask(now, plan.settings.lifetime);
    if expired != 0 {
        return Err(AmpduStartError::Expired(ExpiredAmpdu {
            buffer,
            plan,
            msdus: expired,
        }));
    }
    let Esp32s31AmpduBuffer {
        mut owner,
        subframes,
        admitted_at,
        ..
    } = buffer;
    let AmpduPlan {
        queue,
        admission,
        settings: plan,
    } = plan;
    let result = (|| {
        let deadline = now
            .checked_add(publication_timeout)
            .ok_or(HtAmpduTxError::ResetRequired)?;
        let cookie = owner.begin()?;
        for subframe in subframes.into_iter().flatten() {
            let layout = AmpduFrameLayout::new(
                0,
                AmpduFrameSize::new(subframe.len, plan.hardware_mic_length),
            )
            .ok_or(HtAmpduTxError::FrameTooLong)?;
            match plan.format {
                AmpduFormat::Ht { rate, .. } => owner.commit_ht(
                    cookie,
                    subframe.backing,
                    HtAmpduFrameRequest::new(layout, 0, rate),
                )?,
                AmpduFormat::He { policy, .. } => owner.commit_he(
                    cookie,
                    subframe.backing,
                    HeAmpduFrameRequest::new(layout, policy),
                )?,
            }
        }
        let aggregate = owner.prepared_aggregate(cookie)?;
        let publication = plan.publication;
        match plan.format {
            AmpduFormat::Ht {
                rate,
                protection_spacing,
            } => {
                let config = ht_ampdu_publication_config(
                    plan.role,
                    HtAmpduPublicationInputs {
                        rate,
                        aggregate_length: aggregate.bytes,
                        subframes: aggregate.subframes,
                        protection_spacing,
                        data_power_primary: publication.data_power_primary,
                        data_power_alternate: publication.data_power_alternate,
                        control: publication.control,
                        aifsn: publication.aifsn,
                        contention_window: publication.contention_window,
                        scheduler_priority: publication.scheduler_priority,
                        packet_priority: publication.packet_priority,
                    },
                )
                .ok_or(HtAmpduTxError::AggregateConfigurationMismatch)?;
                owner.submit(hardware, cookie, plan.queue, config)?;
            }
            AmpduFormat::He { policy, bss_color } => {
                let mut config = HeAmpduTxConfig::new_with_txop(
                    policy.rate(),
                    bss_color,
                    aggregate.bytes,
                    aggregate.subframes,
                    policy.density(),
                    policy.txop_limit(),
                )
                .ok_or(HtAmpduTxError::AggregateConfigurationMismatch)?;
                config.data_power_primary = publication.data_power_primary;
                config.data_power_alternate = publication.data_power_alternate;
                config.control = publication.control;
                config.aifsn = publication.aifsn;
                config.contention_window = publication.contention_window;
                config.interface = plan.role.interface;
                config.scheduler_priority = publication.scheduler_priority;
                config.pti = publication.packet_priority;
                config.pti_count = 1;
                config.hardware_key_selector = plan.role.hardware_key_selector;
                owner.submit_he(hardware, cookie, plan.queue, config)?;
            }
        }
        Ok((cookie, deadline))
    })();
    match result {
        Ok((cookie, deadline)) => Ok(PublishedAmpdu {
            owner,
            cookie,
            deadline,
            settling: false,
            admitted_at,
            queue,
            admission,
            end: None,
        }),
        Err(error) => Err(AmpduStartError::Fault(AmpduFault {
            error,
            _owner: owner,
            _queue: queue,
            _admission: admission,
            _admitted_at: admitted_at,
        })),
    }
}

/// Consume one interrupt or deadline edge several queues share, handing
/// the subframes' buffers to `returned` in subframe order once the
/// hardware left the chain, as
/// `OrdinaryTxOwner::service_queued_single_attempt` does for an MPDU: the
/// queue's BlockAck completion, then its collision detach on a collision
/// edge, then its timeout abort on a timeout edge or the expired deadline,
/// begun only while no other queue's abort forces CCA.
/// Diagnostic-only completion does not consume the step: latched events and
/// an expired watchdog remain actionable even on an interrupt wake.
///
/// The publication is served where it lies: only its end moves it, through
/// [`PublishedAmpdu::into_progress`]. An ended or failed publication
/// touches no hardware again and answers as before.
///
/// # Errors
///
/// The step failed; [`PublishedAmpdu::into_progress`] then retains the
/// publication in quarantine.
pub fn service<B, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    published: &mut PublishedAmpdu<'_, '_, B, SLOTS, Path>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: oer_time::Instant,
    returned: impl FnMut(B),
) -> Result<AmpduServed, HtAmpduTxError>
where
    B: PlacedBacking,
    H: HtAmpduHardware,
{
    match published.end {
        Some(Ok(_)) => return Ok(AmpduServed::Ended),
        Some(Err(error)) => return Err(error),
        None => {}
    }
    match service_step(
        hardware,
        published,
        wake,
        may_begin_timeout_abort,
        now,
        returned,
    ) {
        Ok(None) => Ok(AmpduServed::Pending),
        Ok(Some(outcome)) => {
            published.end = Some(Ok(outcome));
            Ok(AmpduServed::Ended)
        }
        Err(error) => {
            published.end = Some(Err(error));
            Err(error)
        }
    }
}

fn service_step<B, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    published: &mut PublishedAmpdu<'_, '_, B, SLOTS, Path>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: oer_time::Instant,
    mut returned: impl FnMut(B),
) -> Result<Option<AmpduOutcome>, HtAmpduTxError>
where
    B: PlacedBacking,
    H: HtAmpduHardware,
{
    use crate::mac::irq::EVENT_TX_TIMEOUT;
    let cookie = published.cookie;
    if published.settling {
        if now < published.deadline {
            return Ok(None);
        }
        published
            .owner
            .finish_timeout_abort(hardware, cookie, &mut returned)?;
        return Ok(Some(AmpduOutcome::Aborted));
    }
    if let Some(event) = published.owner.acknowledge_completion(hardware)? {
        match event {
            crate::mac::tx::CompletionObservation::Observed(completion) => {
                published.owner.detach_completed(hardware, cookie)?;
                published.owner.release_completed(cookie, &mut returned)?;
                return Ok(Some(AmpduOutcome::Completed(completion)));
            }
            crate::mac::tx::CompletionObservation::NoEffect => {}
        }
    }
    let events = match wake {
        WifiTxWake::Interrupt { events } => events,
        WifiTxWake::Deadline => 0,
    };
    if published
        .owner
        .abort_collision(hardware, cookie, &mut returned)?
    {
        return Ok(Some(AmpduOutcome::Collision));
    }
    let expired = now >= published.deadline;
    if (events & EVENT_TX_TIMEOUT != 0 || expired) && may_begin_timeout_abort {
        if published.owner.begin_timeout_abort(hardware, cookie)? {
            published.settling = true;
            published.deadline = now
                .checked_add(TX_ABORT_SETTLE)
                .ok_or(HtAmpduTxError::ResetRequired)?;
            return Ok(None);
        }
        if expired {
            published.owner.require_reset(cookie)?;
            return Err(HtAmpduTxError::ResetRequired);
        }
    }
    Ok(None)
}
