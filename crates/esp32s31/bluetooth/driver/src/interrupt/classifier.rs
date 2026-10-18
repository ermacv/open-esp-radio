//! Evidence-bounded classification of the primary Bluetooth MAC IRQ suffix.
//!
//! The restricted PAC owns the exact masked-status sample and W1C prefix.
//! This module first rejects the four baseline groups consumed by the complete
//! assertion prefix, then classifies the three dynamic source groups consumed
//! by the source-124 scheduler suffix. Raw status and diagnostic images remain
//! inside the PAC; this layer receives positional source facts only.
//!
//! The reference handler can observe `SCHEDULER_STATE` twice: once while
//! handling bank-one source 3 and again while constructing scheduler work.
//! Distinct input types preserve those two temporal positions. A future hard
//! handler must obtain each observation at its named point; it must not reuse
//! one register image for both merely because their bit geometry is equal.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::bluetooth::{BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch};

pub use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
};

/// Dynamic scheduler trigger selected from one acknowledged primary snapshot.
///
/// Names deliberately remain positional. Current evidence proves the branch
/// geometry and scheduler-work effects, but not Bluetooth Link-Layer event
/// names for the individual hardware bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimarySchedulerTrigger {
    /// None of the three reviewed dynamic groups is present.
    None,
    /// Bank-zero source 21 is present without source 27 or 28.
    Bank0Source21,
    /// Bank-zero source 27 or 28 is present.
    Bank0Sources27Or28 {
        /// Whether source 21 was present in the same acknowledged image.
        source_21_pending: bool,
    },
    /// Bank-one source 3 is present and takes precedence over the bank-zero
    /// branch, while retaining whether source 27 or 28 can mark the work.
    Bank1Source3 {
        /// Whether bank-zero source 27 or 28 was also present.
        bank_0_sources_27_or_28_pending: bool,
    },
}

impl PrimarySchedulerTrigger {
    const fn from_epoch(epoch: &BluetoothPrimaryInterruptEpoch) -> Self {
        let sources_27_or_28_pending = epoch.bank_0_sources_27_or_28_pending();

        if epoch.bank_1_source_3_pending() {
            Self::Bank1Source3 {
                bank_0_sources_27_or_28_pending: sources_27_or_28_pending,
            }
        } else if sources_27_or_28_pending {
            Self::Bank0Sources27Or28 {
                source_21_pending: epoch.bank_0_source_21_pending(),
            }
        } else if epoch.bank_0_source_21_pending() {
            Self::Bank0Source21
        } else {
            Self::None
        }
    }

    #[cfg(test)]
    const fn from_dynamic_fields_for_validation(
        source_21_pending: bool,
        sources_27_or_28_pending: bool,
        source_3_pending: bool,
    ) -> Self {
        let epoch = BluetoothPrimaryInterruptEpoch::for_dynamic_validation(
            source_21_pending,
            sources_27_or_28_pending,
            source_3_pending,
        );
        Self::from_epoch(&epoch)
    }

    const fn work_inputs(self) -> Option<(bool, bool)> {
        match self {
            Self::None => None,
            Self::Bank0Source21 => Some((false, true)),
            Self::Bank0Sources27Or28 { source_21_pending } => Some((true, source_21_pending)),
            Self::Bank1Source3 {
                bank_0_sources_27_or_28_pending,
            } => Some((bank_0_sources_27_or_28_pending, true)),
        }
    }
}

/// Classified dynamic suffix of one complete primary interrupt observation.
///
/// The acknowledged PAC epoch remains affine while only semantic scheduler
/// trigger facts are exposed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a classified primary epoch must drive fault or scheduler handling"]
pub struct PrimaryInterruptClassification {
    epoch: BluetoothPrimaryInterruptEpoch,
    scheduler_trigger: PrimarySchedulerTrigger,
}

impl PrimaryInterruptClassification {
    /// Classify one complete restricted-PAC epoch or preserve its fatal path.
    ///
    /// Any baseline fault takes precedence even when dynamic scheduler sources
    /// were pending in the same snapshot. A live Controller must retain the
    /// returned error and enter its fail-stop/quiesce path without publishing
    /// ordinary Link-Layer work for this epoch.
    pub const fn from_epoch(
        epoch: BluetoothPrimaryInterruptEpoch,
    ) -> Result<Self, PrimaryControllerFault> {
        if epoch.fault_sources().is_fault() {
            return Err(PrimaryControllerFault { epoch });
        }
        let scheduler_trigger = PrimarySchedulerTrigger::from_epoch(&epoch);
        Ok(Self {
            epoch,
            scheduler_trigger,
        })
    }

    /// Return the positional dynamic scheduler trigger.
    pub const fn scheduler_trigger(&self) -> PrimarySchedulerTrigger {
        self.scheduler_trigger
    }

    /// Request the first, bank-one-only scheduler-state observation.
    ///
    /// `None` means the complete reference handler does not perform this read.
    pub const fn reference_gate(&self) -> Option<SchedulerReferenceGate> {
        match self.scheduler_trigger {
            PrimarySchedulerTrigger::Bank1Source3 { .. } => Some(SchedulerReferenceGate),
            _ => None,
        }
    }

    /// Request the later scheduler-state observation used to construct work.
    ///
    /// Every reviewed dynamic trigger produces exactly one such request.
    pub const fn work_classifier(&self) -> Option<SchedulerWorkClassifier> {
        match self.scheduler_trigger.work_inputs() {
            Some((mark_candidate, state_publication_requested)) => Some(SchedulerWorkClassifier {
                mark_candidate,
                state_publication_requested,
            }),
            None => None,
        }
    }
}

/// Fatal primary interrupt result retaining semantic source presence.
///
/// The type classifies the reference handler's assertion path; it does not
/// panic in interrupt context. The future Controller lifecycle owns the
/// fail-stop transition, user-visible reset reason and quiescent recovery.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the Controller fault must be retained until fail-stop recovery completes"]
pub struct PrimaryControllerFault {
    epoch: BluetoothPrimaryInterruptEpoch,
}

impl PrimaryControllerFault {
    /// Return the positional fault sources that selected fail-stop handling.
    pub const fn sources(&self) -> BluetoothPrimaryFaultSources {
        self.epoch.fault_sources()
    }
}

/// Classifier token proving that bank-one source 3 requires a reference gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerReferenceGate;

impl SchedulerReferenceGate {
    /// Decide the exact hardware action following the first state read.
    pub const fn classify(
        self,
        observation: BluetoothSchedulerReferenceGateObservation,
    ) -> SchedulerReferenceAction {
        if !observation.is_busy() {
            SchedulerReferenceAction::ClearReferenceAndContinue
        } else {
            SchedulerReferenceAction::PreserveReference
        }
    }
}

/// Required scheduler-reference disposition selected after the first read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerReferenceAction {
    /// Write zero to `SCHEDULER_REFERENCE` before the later work observation.
    ///
    /// The vendor's following selector-6 callback validates fields in its own
    /// intrusive transaction/list implementation. The open DTM scheduler does
    /// not create that container: its sole-item lifecycle is represented by
    /// affine Rust states, so that software-only callback has no applicable
    /// member and is not reproduced.
    ClearReferenceAndContinue,
    /// Leave `SCHEDULER_REFERENCE` unchanged while the busy bit is set.
    PreserveReference,
}

/// Deferred-work classifier retaining the two booleans selected by the ISR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerWorkClassifier {
    mark_candidate: bool,
    state_publication_requested: bool,
}

impl SchedulerWorkClassifier {
    /// Classify the second scheduler-state observation into one worker wake.
    pub const fn classify(
        self,
        observation: &BluetoothSchedulerWorkObservation,
    ) -> SchedulerWorkerWake {
        let deferred_work_requested = observation.deferred_work_requested();
        SchedulerWorkerWake {
            class: if self.mark_candidate && deferred_work_requested {
                SchedulerWorkerWakeClass::Marked
            } else {
                SchedulerWorkerWakeClass::Ordinary
            },
            deferred_work_publication: if self.state_publication_requested {
                Some(deferred_work_requested)
            } else {
                None
            },
        }
    }
}

/// Sticky class carried by the single deferred scheduler work item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWorkerWakeClass {
    /// Process scheduler work without setting the deferred-work marker.
    Ordinary,
    /// Preserve that the deferred-work predicate was true when a mark-capable
    /// source fired. The marker remains sticky if wakes are coalesced.
    Marked,
}

/// One deferred scheduler-worker wake derived from a dynamic primary IRQ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerWorkerWake {
    class: SchedulerWorkerWakeClass,
    deferred_work_publication: Option<bool>,
}

impl SchedulerWorkerWake {
    /// Whether this wake carries the sticky deferred-work marker.
    pub const fn class(self) -> SchedulerWorkerWakeClass {
        self.class
    }

    /// Optional combined deferred-work value published by the reviewed path.
    ///
    /// This records the exact binary behavior needed to evaluate a replacement
    /// worker. It does not make the vendor callback selector an open-driver ABI.
    pub const fn deferred_work_publication(self) -> Option<bool> {
        self.deferred_work_publication
    }
}

#[cfg(test)]
mod tests;
