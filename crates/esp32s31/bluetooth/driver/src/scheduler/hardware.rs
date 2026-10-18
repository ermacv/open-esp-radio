//! Hardware execution of the scheduler executor's list-zero steps.
//!
//! [`SchedulerHardware`] performs the [`SchedulerAction`]s of one executor
//! step in order, takes one fresh observation for each awaited
//! [`SchedulerWait`] and starts an idle scheduler at a published head. It
//! retains every affine HAL publication between the action that creates it
//! and the action that ends it, so an action out of transaction order fails
//! closed before any register changes.
//!
//! A wait observation joins a fresh task-side read with a fresh scheduler
//! state read from the stable interrupt owner. The scheduler wake only tells
//! the caller when to observe again; it never carries the observed value.

#![deny(unsafe_code)]

use oer_esp32s31_bluetooth_memory::ControllerSramLinkAddress;

use crate::scheduler::{
    SchedulerAction, SchedulerIdleInsertion, SchedulerObservation, SchedulerTransactionFault,
    SchedulerWait,
};

/// Affine proofs of the list-zero publications, one type each.
pub(crate) trait SchedulerPublications {
    type Lock;
    type Modify;
    type LockModify;
    type Indexed;
    type Acknowledged;
    type Cancellation;
    type Skip;
    type Run;
}

type Lock<B> = <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Lock;
type Modify<B> = <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Modify;
type LockModify<B> =
    <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::LockModify;
type Indexed<B> = <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Indexed;
type Acknowledged<B> =
    <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Acknowledged;
type Cancellation<B> =
    <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Cancellation;
type Skip<B> = <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Skip;
type Run<B> = <<B as SchedulerHardwareBackend>::Publications as SchedulerPublications>::Run;

/// Register operations of list zero for the duration of one call.
///
/// The live backend joins the task HAL owner with the stable interrupt owner.
pub(crate) trait SchedulerHardwareBackend {
    type Publications: SchedulerPublications;
    type StartError;

    fn publish_execution_lock(&mut self, at: ControllerSramLinkAddress) -> Lock<Self>;
    fn release_execution_lock(&mut self, lock: Lock<Self>);
    fn publish_execution_modify(&mut self, list_deletion: bool) -> Modify<Self>;
    fn release_execution_modify(&mut self, modify: Modify<Self>);
    fn publish_lock_modify(&mut self, at: ControllerSramLinkAddress) -> LockModify<Self>;
    fn publish_head(&mut self, head: Option<ControllerSramLinkAddress>);
    fn index_cancellation(&mut self) -> Indexed<Self>;
    fn acknowledge_cancellation_source(
        &mut self,
        indexed: &Indexed<Self>,
    ) -> Result<Acknowledged<Self>, SchedulerHardwareError>;
    fn request_cancellation(
        &mut self,
        indexed: Indexed<Self>,
        acknowledged: Acknowledged<Self>,
    ) -> Cancellation<Self>;
    fn release_cancellation(&mut self, cancellation: Cancellation<Self>);
    fn publish_skip(&mut self, at: ControllerSramLinkAddress) -> Skip<Self>;
    fn clear_skip(&mut self, skip: Skip<Self>);

    fn observe(
        &mut self,
        wait: SchedulerWait,
        lock: Option<&mut Lock<Self>>,
        modify: Option<&mut Modify<Self>>,
        cancellation: Option<&mut Cancellation<Self>>,
        skip: Option<&Skip<Self>>,
    ) -> Result<SchedulerObservation, SchedulerHardwareError>;

    fn start(
        &mut self,
        head: ControllerSramLinkAddress,
    ) -> Result<Run<Self>, SchedulerStartError<Self::StartError>>;
}

/// Why a step or observation was refused. The retained publications are
/// unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerHardwareError {
    /// The action does not continue the publications in progress.
    OutOfOrder(SchedulerAction),
    /// The awaited publication is not in progress.
    NotAwaiting(SchedulerWait),
    /// Platform storage does not hold the stable interrupt owner.
    InterruptOwnerUnavailable,
    /// An action names a link that is no submitted item of the item space.
    ForeignItem(ControllerSramLinkAddress),
    /// The hardware head does not decode to a controller link.
    ForeignHead,
    /// A scheduler diagnostic sample never settled within
    /// [`DIAGNOSTIC_READ_BUDGET`](super::DIAGNOSTIC_READ_BUDGET).
    DiagnosticUnsettled,
}

/// Why the scheduler stop sequence ended before the scheduler stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerStopError {
    /// Platform storage does not hold the stable interrupt owner.
    InterruptOwnerUnavailable,
    /// A BUSY sample never settled within
    /// [`DIAGNOSTIC_READ_BUDGET`](super::DIAGNOSTIC_READ_BUDGET).
    DiagnosticUnsettled,
}

/// Why an idle scheduler was not started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerStartError<E> {
    /// A list transaction still holds a publication.
    TransactionActive,
    /// The head is no submitted item of the item space.
    ForeignItem(ControllerSramLinkAddress),
    /// The interrupt owner could not prepare the run interrupts. The head is
    /// published; a later start publishes it again.
    Interrupts(E),
}

/// Publications of the list transaction in progress and the latest RUN.
pub(crate) struct SchedulerHardware<P: SchedulerPublications> {
    lock: Option<P::Lock>,
    modify: Option<P::Modify>,
    lock_modify: Option<P::LockModify>,
    indexed: Option<P::Indexed>,
    acknowledged: Option<P::Acknowledged>,
    cancellation: Option<P::Cancellation>,
    skip: Option<P::Skip>,
    run: Option<P::Run>,
}

impl<P: SchedulerPublications> SchedulerHardware<P> {
    pub(crate) const fn new() -> Self {
        Self {
            lock: None,
            modify: None,
            lock_modify: None,
            indexed: None,
            acknowledged: None,
            cancellation: None,
            skip: None,
            run: None,
        }
    }

    /// Whether no list transaction holds a publication.
    pub(crate) const fn is_quiet(&self) -> bool {
        self.lock.is_none()
            && self.modify.is_none()
            && self.lock_modify.is_none()
            && self.indexed.is_none()
            && self.acknowledged.is_none()
            && self.cancellation.is_none()
            && self.skip.is_none()
    }

    /// Whether a RUN command was published in this epoch.
    pub(crate) const fn has_run(&self) -> bool {
        self.run.is_some()
    }

    /// Perform the actions of one executor step, in order.
    ///
    /// Every action is validated before the first one runs, so a refused step
    /// changes no register. An interrupt owner missing from storage stops the
    /// step after the actions already performed; the epoch is then faulted,
    /// because storage holds that owner for as long as the scheduler runs.
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn perform<
        I: Copy,
        const CAPACITY: usize,
        B: SchedulerHardwareBackend<Publications = P>,
    >(
        &mut self,
        backend: &mut B,
        step: &crate::scheduler::SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError> {
        self.perform_actions(backend, step.actions())
    }

    fn perform_actions<B: SchedulerHardwareBackend<Publications = P>>(
        &mut self,
        backend: &mut B,
        actions: impl Iterator<Item = SchedulerAction> + Clone,
    ) -> Result<(), SchedulerHardwareError> {
        self.admit(actions.clone())?;
        for action in actions {
            self.run_action(backend, action)?;
        }
        Ok(())
    }

    /// Clear what a faulted transaction left published, as the executor
    /// fault requires.
    pub(crate) fn recover<B: SchedulerHardwareBackend<Publications = P>>(
        &mut self,
        backend: &mut B,
        fault: SchedulerTransactionFault,
    ) {
        match fault {
            SchedulerTransactionFault::UnsupportedExecutionLockResult => {
                if let Some(lock) = self.lock.take() {
                    backend.release_execution_lock(lock);
                }
            }
            SchedulerTransactionFault::ExecutionModifyRejected => {
                if let Some(modify) = self.modify.take() {
                    backend.release_execution_modify(modify);
                }
            }
            SchedulerTransactionFault::UnsupportedSkipResult => {
                if let Some(skip) = self.skip.take() {
                    backend.clear_skip(skip);
                }
                if let Some(cancellation) = self.cancellation.take() {
                    backend.release_cancellation(cancellation);
                }
            }
            SchedulerTransactionFault::NoTransaction
            | SchedulerTransactionFault::UnexpectedObservation => {}
        }
    }

    /// Take one fresh observation of the awaited publication.
    pub(crate) fn observe<B: SchedulerHardwareBackend<Publications = P>>(
        &mut self,
        backend: &mut B,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        let awaiting = match wait {
            SchedulerWait::ExecutionLock => self.lock.is_some(),
            SchedulerWait::ExecutionModify => self.modify.is_some(),
            // Cancellation also waits for a request of an earlier insertion
            // to leave START, so this wait needs no publication of its own.
            SchedulerWait::LockModify => true,
            SchedulerWait::Cancellation => self.cancellation.is_some(),
            SchedulerWait::Skip => self.skip.is_some(),
        };
        if !awaiting {
            return Err(SchedulerHardwareError::NotAwaiting(wait));
        }
        let observation = backend.observe(
            wait,
            self.lock.as_mut(),
            self.modify.as_mut(),
            self.cancellation.as_mut(),
            self.skip.as_ref(),
        )?;
        if let SchedulerObservation::LockModify(observed) = observation
            && !observed.wait_active()
        {
            // The request left START; its publication has ended.
            self.lock_modify = None;
        }
        Ok(observation)
    }

    /// Publish `insertion`'s head and start the idle scheduler with RUN.
    pub(crate) fn start<B: SchedulerHardwareBackend<Publications = P>>(
        &mut self,
        backend: &mut B,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<B::StartError>> {
        if !self.is_quiet() {
            return Err(SchedulerStartError::TransactionActive);
        }
        self.run = Some(backend.start(insertion.head)?);
        Ok(())
    }

    fn admit(
        &self,
        actions: impl Iterator<Item = SchedulerAction>,
    ) -> Result<(), SchedulerHardwareError> {
        let mut state = Admission {
            lock: self.lock.is_some(),
            modify: self.modify.is_some(),
            lock_modify: self.lock_modify.is_some(),
            indexed: self.indexed.is_some(),
            acknowledged: self.acknowledged.is_some(),
            cancellation: self.cancellation.is_some(),
            skip: self.skip.is_some(),
        };
        for action in actions {
            if !state.apply(action) {
                return Err(SchedulerHardwareError::OutOfOrder(action));
            }
        }
        Ok(())
    }

    fn run_action<B: SchedulerHardwareBackend<Publications = P>>(
        &mut self,
        backend: &mut B,
        action: SchedulerAction,
    ) -> Result<(), SchedulerHardwareError> {
        match action {
            SchedulerAction::PublishExecutionLock(at) => {
                self.lock = Some(backend.publish_execution_lock(at));
            }
            SchedulerAction::ReleaseExecutionLock => {
                backend.release_execution_lock(self.lock.take().expect("admitted"));
            }
            SchedulerAction::PublishExecutionModify => {
                self.modify = Some(backend.publish_execution_modify(false));
            }
            SchedulerAction::PublishExecutionModifyListDeletion => {
                self.modify = Some(backend.publish_execution_modify(true));
            }
            SchedulerAction::ReleaseExecutionModify => {
                backend.release_execution_modify(self.modify.take().expect("admitted"));
            }
            SchedulerAction::PublishLockModify(at) => {
                self.lock_modify = Some(backend.publish_lock_modify(at));
            }
            SchedulerAction::PublishHead(head) => backend.publish_head(head),
            SchedulerAction::IndexCancellation => {
                self.indexed = Some(backend.index_cancellation());
            }
            SchedulerAction::AcknowledgeCancellationSource => {
                let indexed = self.indexed.as_ref().expect("admitted");
                self.acknowledged = Some(backend.acknowledge_cancellation_source(indexed)?);
            }
            SchedulerAction::RequestCancellation => {
                let indexed = self.indexed.take().expect("admitted");
                let acknowledged = self.acknowledged.take().expect("admitted");
                self.cancellation = Some(backend.request_cancellation(indexed, acknowledged));
            }
            SchedulerAction::PublishSkip(at) => {
                self.skip = Some(backend.publish_skip(at));
            }
            SchedulerAction::ClearSkip => {
                backend.clear_skip(self.skip.take().expect("admitted"));
            }
            SchedulerAction::ReleaseCancellation => {
                backend.release_cancellation(self.cancellation.take().expect("admitted"));
            }
        }
        Ok(())
    }
}

/// Which publications exist while a step is validated.
struct Admission {
    lock: bool,
    modify: bool,
    lock_modify: bool,
    indexed: bool,
    acknowledged: bool,
    cancellation: bool,
    skip: bool,
}

impl Admission {
    /// Apply one action; `false` when it does not continue the publications.
    fn apply(&mut self, action: SchedulerAction) -> bool {
        fn open(slot: &mut bool) -> bool {
            !core::mem::replace(slot, true)
        }
        fn close(slot: &mut bool) -> bool {
            core::mem::replace(slot, false)
        }
        match action {
            SchedulerAction::PublishExecutionLock(_) => open(&mut self.lock),
            SchedulerAction::ReleaseExecutionLock => {
                // A lock-modify request ends at its terminal observation,
                // which comes before this release.
                !self.lock_modify && close(&mut self.lock)
            }
            SchedulerAction::PublishExecutionModify
            | SchedulerAction::PublishExecutionModifyListDeletion => open(&mut self.modify),
            SchedulerAction::ReleaseExecutionModify => close(&mut self.modify),
            SchedulerAction::PublishLockModify(_) => self.lock && open(&mut self.lock_modify),
            SchedulerAction::PublishHead(_) => true,
            SchedulerAction::IndexCancellation => !self.cancellation && open(&mut self.indexed),
            SchedulerAction::AcknowledgeCancellationSource => {
                self.indexed && open(&mut self.acknowledged)
            }
            SchedulerAction::RequestCancellation => {
                close(&mut self.indexed)
                    && close(&mut self.acknowledged)
                    && open(&mut self.cancellation)
            }
            SchedulerAction::PublishSkip(_) => self.cancellation && open(&mut self.skip),
            SchedulerAction::ClearSkip => close(&mut self.skip),
            SchedulerAction::ReleaseCancellation => !self.skip && close(&mut self.cancellation),
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) mod live;

#[cfg(test)]
mod tests;
