//! The A-MPDU hardware sequence of the lower-MAC core.
//!
//! One aggregate is one attempt: the core plans it
//! ([`LowerMacCore::plan_ampdu`](super::LowerMacCore::plan_ampdu)),
//! [`publish`] has [`RetainedDmaAmpduTx`] begin it, commit every subframe
//! with `commit_ht` or `commit_he` and publish it once, and [`service`]
//! ends it. Its completion reports the recipient's BlockAck; the aggregate
//! is never retained for a retry (`retain_for_ampdu_retry`), since
//! selecting what a later aggregate retransmits is the portable layer's.
//!
//! The backend's driver does not submit aggregates yet: A-MPDU
//! transmission is not in the first image of the ESP32-S31 backend; it will
//! follow the phase contract's form (`PhaseAggregate`, numbered subframes).
//!
//! # Buffers
//!
//! A lent [`Esp32s31AmpduBuffer`] is one idle aggregate owner together with
//! the subframes the caller appended. Each subframe is a
//! [`PlacedBacking`] lease of the integrator's [`AmpduBackingSource`],
//! the same retained-lease memory the station and access-point aggregate
//! paths publish from: the MPDU is written after the eight-byte metadata
//! prefix at the start of the lease's region, and `commit_ht` publishes it
//! where it was written. The owner retains every lease until the attempt's
//! completion releases it, when dropping the lease returns it to its source.
//! The ordinary TX slots cannot serve here: each is a single descriptor with
//! its own buffer, while an aggregate descriptor chain references leases.
//!
//! # Limits
//!
//! HT and HE SU, at most `SLOTS` subframes and [`ESP32S31_AMPDU_MAX_LENGTH`]
//! octets. The HE path carries the recipient's density and the access
//! category's TXOP limit in its checked plan. Trigger-based aggregates stay
//! outside the declared formats; they require a separate Trigger reservation.

use super::{FreeQueue, TxAdmission};
use core::fmt;
use oer_radio_port::{PoolPath, Root};

use oer_esp32s31_hal::dma_static::{PlacedBacking, PlacedTxLease};
use oer_esp32s31_ieee80211_mac::tx::{
    HeAmpduTxConfig, HtAmpduDensity, HtChannelWidth, HtGuardInterval, HtMcs, HtProtectionSpacing,
    HtRate, LegacyTxQueue, TxControlFrame, TxCookie,
    ampdu::{
        AmpduFrameLayout, AmpduFrameSize, HeAmpduFrameRequest, HeAmpduPolicy, HtAmpduFrameRequest,
        HtAmpduHardware, HtAmpduTxCompletion, HtAmpduTxError, RetainedDmaAmpduTx,
        TX_AMPDU_METADATA_SIZE,
    },
};
use oer_ieee80211_port::{AmpduCapabilities, BlockAckReport, PhyFormatSet, TxStatus};
use oer_memory::{PinnedDmaTxRadioLease, StableDmaBacking as _};

use crate::{
    ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config},
    ordinary_tx::{TX_ABORT_SETTLE, TX_CCMP_MIC_SIZE, TX_FCS_SIZE},
    tx::WifiTxWake,
};

/// The longest aggregate the core publishes: the vendor's per-rate ceiling
/// of HT MCS 0 with the long guard interval (`rx11NRate2AMPDULimit`, see
/// [`HtRate::vendor_ampdu_byte_limit`]), the lowest of every HT rate and
/// below the 8191 octets every HT recipient accepts. The aggregate owner is
/// configured to it, so an aggregate is refused exactly when it is longer.
pub const ESP32S31_AMPDU_MAX_LENGTH: u16 = match HtRate::new(
    HtMcs::Mcs0,
    HtGuardInterval::Long800Ns,
    HtChannelWidth::Mhz20,
)
.vendor_ampdu_byte_limit()
{
    Some(limit) => limit,
    None => 0x1fff,
};

/// The limits of the core's aggregates of up to `slots` subframes.
pub const fn esp32s31_ampdu_capabilities(slots: usize) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: if slots > u16::MAX as usize {
            u16::MAX
        } else {
            slots as u16
        },
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: ESP32S31_AMPDU_MAX_LENGTH as u32,
    }
}

/// The memory type of aggregate subframes: what an aggregate owner of the
/// core retains.
pub trait AmpduBacking {
    /// One subframe's memory. Its region must start word-aligned, as every
    /// S31 TX descriptor buffer does.
    type Backing: PlacedBacking;
}

/// Stable DMA memory the core lends as aggregate subframes.
///
/// Dropping a backing returns it to its source, as
/// `oer_memory::ReturningStableDmaBacking` returns a pool slot.
pub trait AmpduBackingSource: AmpduBacking {
    /// A free backing, or `None` when every one is in use.
    fn backing(&self) -> Option<Self::Backing>;
}

/// The subframe memory of a core without aggregates. It is no
/// [`AmpduBackingSource`], so such a core lends no aggregate and its port
/// does not implement `LowerMacAmpdu`; its backing type is never
/// constructed.
#[derive(Debug)]
pub enum NoAmpdu {}

impl AmpduBacking for NoAmpdu {
    type Backing = PlacedTxLease<PinnedDmaTxRadioLease<'static, 0, 0, 0>>;
}

/// The aggregate owner behind one lent buffer. The port publishes only
/// referenced subframes, so its formatter workspace is empty.
pub type Esp32s31AmpduOwner<'slot, B, const SLOTS: usize> = RetainedDmaAmpduTx<'slot, B, SLOTS>;

struct Subframe<B> {
    backing: B,
    len: usize,
}

/// One aggregate being built: an idle aggregate owner and the subframes
/// appended so far. [`publish`] takes it; [`Self::into_owner`] gives the
/// owner back unpublished, its subframes returned to their source.
pub struct Esp32s31AmpduBuffer<'slot, S: AmpduBacking, const SLOTS: usize> {
    pub(super) owner: Esp32s31AmpduOwner<'slot, S::Backing, SLOTS>,
    source: &'slot S,
    subframes: [Option<Subframe<S::Backing>>; SLOTS],
    count: usize,
}

impl<'slot, S: AmpduBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'slot, S, SLOTS> {
    /// An empty aggregate over the idle `owner`, its subframes from
    /// `source`, the owner configured to [`ESP32S31_AMPDU_MAX_LENGTH`].
    ///
    /// # Errors
    ///
    /// The owner, when it is not idle.
    #[allow(clippy::result_large_err, reason = "the owner goes back by value")]
    pub fn new(
        mut owner: Esp32s31AmpduOwner<'slot, S::Backing, SLOTS>,
        source: &'slot S,
    ) -> Result<Self, Esp32s31AmpduOwner<'slot, S::Backing, SLOTS>> {
        if owner
            .configure_max_aggregate_bytes(ESP32S31_AMPDU_MAX_LENGTH)
            .is_err()
        {
            return Err(owner);
        }
        Ok(Self {
            owner,
            source,
            subframes: [const { None }; SLOTS],
            count: 0,
        })
    }

    /// The appended MPDU lengths, in order.
    pub fn lengths(&self) -> impl Iterator<Item = usize> + '_ {
        self.subframes.iter().flatten().map(|subframe| subframe.len)
    }

    /// Return the subframes to their source and give the owner back.
    pub fn into_owner(self) -> Esp32s31AmpduOwner<'slot, S::Backing, SLOTS> {
        self.owner
    }
}

impl<S: AmpduBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'_, S, SLOTS> {
    pub(super) fn matches_admission<Path: PoolPath>(
        &mut self,
        role: oer_ieee80211_port::VifRole,
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

impl<S: AmpduBackingSource, const SLOTS: usize> Esp32s31AmpduBuffer<'_, S, SLOTS> {
    /// Append a subframe of `len` MPDU octets: the memory to encode it into.
    /// `None` when the aggregate holds `SLOTS`, the length is zero, the
    /// source has no free backing, or its backing is too short for the MPDU
    /// with its metadata, MIC and FCS.
    pub fn push_mpdu(&mut self, len: usize) -> Option<&mut [u8]> {
        if self.count >= SLOTS || len == 0 {
            return None;
        }
        let capacity = subframe_capacity(len)?;
        let mut backing = self.source.backing()?;
        if backing.stable_dma_region().len() < capacity {
            // Dropping the backing returns it.
            return None;
        }
        let subframe = self
            .subframes
            .get_mut(self.count)?
            .insert(Subframe { backing, len });
        self.count += 1;
        let region = subframe.backing.stable_dma_region().into_mut_slice();
        region.get_mut(TX_AMPDU_METADATA_SIZE..TX_AMPDU_METADATA_SIZE + len)
    }
}

impl<S: AmpduBacking, const SLOTS: usize> Esp32s31AmpduBuffer<'_, S, SLOTS> {
    /// The subframes appended.
    pub const fn subframes(&self) -> usize {
        self.count
    }
}

impl<S: AmpduBacking, const SLOTS: usize> fmt::Debug for Esp32s31AmpduBuffer<'_, S, SLOTS> {
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
pub struct AmpduPlan<'core, Path: PoolPath = Root> {
    pub(super) queue: FreeQueue<'core, Path>,
    pub(super) admission: TxAdmission<'core, Path>,
    pub(super) settings: AmpduSettings,
}

/// Register values already derived from the owned admission.
pub(super) struct AmpduSettings {
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
    HtProtectionSpacing::from_density(HtAmpduDensity::from_ampdu_parameters(
        (min_mpdu_start_spacing & 0x07) << 2,
    ))
}

/// A published aggregate on its queue.
pub struct PublishedAmpdu<'core, 'slot, B, const SLOTS: usize, Path: PoolPath = Root> {
    queue: FreeQueue<'core, Path>,
    admission: TxAdmission<'core, Path>,
    owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    cookie: TxCookie,
    /// The publication deadline, or the end of the abort settle.
    deadline: oer_time::Instant,
    settling: bool,
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
}

/// How a published aggregate ended.
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

pub enum AmpduProgress<'core, 'slot, B, const SLOTS: usize, Path: PoolPath = Root> {
    Pending(PublishedAmpdu<'core, 'slot, B, SLOTS, Path>),
    /// The aggregate ended and released its subframes; the owner is idle.
    Complete(CompletedAmpdu<'core, 'slot, B, SLOTS, Path>),
}

/// The detached aggregate and its affine terminal. Only the installing core
/// releases the queue and key right while publishing the terminal to its sink.
#[must_use = "return the detached aggregate through its core"]
pub struct CompletedAmpdu<'core, 'slot, B, const SLOTS: usize, Path: PoolPath = Root> {
    pub(super) outcome: AmpduOutcome,
    pub(super) owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    pub(super) queue: FreeQueue<'core, Path>,
    pub(super) admission: TxAdmission<'core, Path>,
}

/// An aggregate fault retains every admitted right and its DMA owner. It has
/// no release operation: a poisoned backend carries it through reset custody.
#[must_use = "retain the poisoned aggregate in the backend's quarantine"]
pub struct AmpduFault<'core, 'slot, B, const SLOTS: usize, Path: PoolPath = Root> {
    pub error: HtAmpduTxError,
    _owner: Esp32s31AmpduOwner<'slot, B, SLOTS>,
    _queue: FreeQueue<'core, Path>,
    _admission: TxAdmission<'core, Path>,
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

/// Commit the buffer's subframes to its owner and publish the aggregate
/// once. An error returns the owner and its admission in sealed quarantine.
#[allow(
    clippy::result_large_err,
    reason = "retain the complete DMA and key owners without allocation"
)]
pub fn publish<'core, 'slot, S, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    buffer: Esp32s31AmpduBuffer<'slot, S, SLOTS>,
    plan: AmpduPlan<'core, Path>,
    now: oer_time::Instant,
    publication_timeout: oer_time::Duration,
) -> Result<
    PublishedAmpdu<'core, 'slot, S::Backing, SLOTS, Path>,
    AmpduFault<'core, 'slot, S::Backing, SLOTS, Path>,
>
where
    S: AmpduBacking,
    H: HtAmpduHardware,
{
    let Esp32s31AmpduBuffer {
        mut owner,
        subframes,
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
            queue,
            admission,
        }),
        Err(error) => Err(AmpduFault {
            error,
            _owner: owner,
            _queue: queue,
            _admission: admission,
        }),
    }
}

/// Consume one interrupt or deadline edge several queues share, as
/// `OrdinaryTxOwner::service_queued_single_attempt` does for an MPDU: the
/// queue's BlockAck completion, then its collision detach on a collision
/// edge, then its timeout abort on a timeout edge or the expired deadline,
/// begun only while no other queue's abort forces CCA.
/// Diagnostic-only completion does not consume the step: latched events and
/// an expired watchdog remain actionable even on an interrupt wake.
#[allow(
    clippy::result_large_err,
    reason = "a fault retains the complete publication in quarantine"
)]
pub fn service<'core, 'slot, B, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    mut published: PublishedAmpdu<'core, 'slot, B, SLOTS, Path>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: oer_time::Instant,
) -> Result<AmpduProgress<'core, 'slot, B, SLOTS, Path>, AmpduFault<'core, 'slot, B, SLOTS, Path>>
where
    B: PlacedBacking,
    H: HtAmpduHardware,
{
    match service_step(hardware, &mut published, wake, may_begin_timeout_abort, now) {
        Ok(None) => Ok(AmpduProgress::Pending(published)),
        Ok(Some(outcome)) => Ok(AmpduProgress::Complete(CompletedAmpdu {
            outcome,
            owner: published.owner,
            queue: published.queue,
            admission: published.admission,
        })),
        Err(error) => Err(AmpduFault {
            error,
            _owner: published.owner,
            _queue: published.queue,
            _admission: published.admission,
        }),
    }
}

fn service_step<B, H, const SLOTS: usize, Path: PoolPath>(
    hardware: &mut H,
    published: &mut PublishedAmpdu<'_, '_, B, SLOTS, Path>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: oer_time::Instant,
) -> Result<Option<AmpduOutcome>, HtAmpduTxError>
where
    B: PlacedBacking,
    H: HtAmpduHardware,
{
    use oer_esp32s31_ieee80211_mac::irq::EVENT_TX_TIMEOUT;
    let cookie = published.cookie;
    if published.settling {
        if now < published.deadline {
            return Ok(None);
        }
        published.owner.finish_timeout_abort(hardware, cookie)?;
        return Ok(Some(AmpduOutcome::Aborted));
    }
    if let Some(event) = published.owner.acknowledge_completion(hardware)? {
        match event {
            oer_esp32s31_ieee80211_mac::tx::CompletionObservation::Observed(completion) => {
                published.owner.detach_completed(hardware, cookie)?;
                published.owner.release_completed(cookie)?;
                return Ok(Some(AmpduOutcome::Completed(completion)));
            }
            oer_esp32s31_ieee80211_mac::tx::CompletionObservation::NoEffect => {}
        }
    }
    let events = match wake {
        WifiTxWake::Interrupt { events } => events,
        WifiTxWake::Deadline => 0,
    };
    if published.owner.abort_collision(hardware, cookie)? {
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
