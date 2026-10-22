//! Address publication and completion return by the arena's single owner.

use super::scheduler::{
    SchedulerCancelError, SchedulerHardwareError, SchedulerNext, SchedulerRunInterruptStorage,
    SchedulerStartError, SchedulerStep, SchedulerSubmitError, SchedulerTransactionFault,
    SchedulerWait,
};
use super::{arena::Engine, *};
use crate::settle::Settle;

#[derive(Debug)]
pub enum EngineScheduleError<E> {
    Pool(SchedulerPoolError),
    Submit(SchedulerSubmitError<SchedulerItemId>),
    Cancel(SchedulerCancelError<SchedulerItemId>),
    Transaction(SchedulerTransactionFault),
    Hardware(SchedulerHardwareError),
    Start(SchedulerStartError<E>),
    NotStopped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceivePublicationError {
    SchedulerStarted,
    NotPublished,
    Scan(crate::bluetooth::ScanStartError),
}

#[derive(Debug)]
pub enum CompletionError {
    WrongList(crate::bluetooth::BluetoothSchedulerFinishedHardwareListObserved),
    TransactionActive(crate::bluetooth::BluetoothSchedulerFinishedHardwareListObserved),
    Pool(SchedulerPoolError),
}

impl<
    'registers,
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> Engine<'registers, 'pool, 'epoch, A, C, N, R, O, Q>
{
    pub fn publish_receive_chains(&mut self) -> Result<(), ReceivePublicationError> {
        if self.hardware.has_run() {
            return Err(ReceivePublicationError::SchedulerStarted);
        }
        self.scanning_publication = Some(
            self.controller
                .publish_rx_memory_list_initial_head(&self.scanning_rx.publication_head()),
        );
        let _ = self
            .controller
            .publish_software_connection_rx_memory_list_initial_head(
                &self.other_rx.publication_head(),
            );
        Ok(())
    }

    pub fn publish_scan_start(&mut self) -> Result<(), ReceivePublicationError> {
        let scanning = self
            .scanning_publication
            .as_ref()
            .ok_or(ReceivePublicationError::NotPublished)?;
        self.controller
            .publish_scan_start(scanning)
            .map(|_| ())
            .map_err(ReceivePublicationError::Scan)
    }

    fn list_item(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        item: usize,
    ) -> Result<SchedulerItemId, SchedulerPoolError> {
        match instance.kind() {
            SchedulerRoleKind::DirectTestMode => self.dtm.submit(instance, item),
            SchedulerRoleKind::LegacyAdvertising => self.advertising.submit(instance, item),
            SchedulerRoleKind::ConnectableAdvertising => self.connectable.submit(instance, item),
            SchedulerRoleKind::LegacyScanning => self.scanning.submit(instance, item),
            SchedulerRoleKind::PeripheralConnection => self.connections.submit(instance, item),
        }
    }

    fn return_item(&mut self, id: SchedulerItemId) -> Result<(), SchedulerPoolError> {
        match id.kind() {
            SchedulerRoleKind::DirectTestMode => self.dtm.retire(id),
            SchedulerRoleKind::LegacyAdvertising => self.advertising.retire(id),
            SchedulerRoleKind::ConnectableAdvertising => self.connectable.retire(id),
            SchedulerRoleKind::LegacyScanning => self.scanning.retire(id),
            SchedulerRoleKind::PeripheralConnection => self.connections.retire(id),
        }
    }

    fn perform<E>(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        step: &SchedulerStep<SchedulerItemId, Q>,
    ) -> Result<SchedulerNext, EngineScheduleError<E>> {
        let items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        self.hardware
            .perform_live(&mut self.controller, storage, &items, step)
            .map_err(EngineScheduleError::Hardware)?;
        for (id, _) in step.released().iter() {
            self.return_item(id).map_err(EngineScheduleError::Pool)?;
        }
        Ok(step.next())
    }

    /// Admit one prepared item. No raw head or physical action escapes HAL.
    pub fn submit<S: SchedulerRunInterruptStorage>(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        item: usize,
        storage: &S,
        clock: &dyn oer_time::Clock,
    ) -> Result<(SchedulerItemId, SchedulerNext), EngineScheduleError<S::Error>> {
        let window = match instance.kind() {
            SchedulerRoleKind::DirectTestMode => self.dtm.prepared_window(instance, item),
            SchedulerRoleKind::LegacyAdvertising => {
                self.advertising.prepared_window(instance, item)
            }
            SchedulerRoleKind::ConnectableAdvertising => {
                self.connectable.prepared_window(instance, item)
            }
            SchedulerRoleKind::LegacyScanning => self.scanning.prepared_window(instance, item),
            SchedulerRoleKind::PeripheralConnection => {
                self.connections.prepared_window(instance, item)
            }
        }
        .map_err(EngineScheduleError::Pool)?;
        let id = self
            .list_item(instance, item)
            .map_err(EngineScheduleError::Pool)?;
        let mut items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let snapshot = match self.hardware.snapshot_live(
            &mut self.controller,
            storage,
            &items,
            Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.return_item(id).map_err(EngineScheduleError::Pool)?;
                return Err(EngineScheduleError::Hardware(error));
            }
        };
        if snapshot.busy.is_busy() {
            let step = match self
                .scheduler
                .begin_live_insertion(&items, &snapshot.busy, id, window)
            {
                Ok(step) => step,
                Err(error) => {
                    self.return_item(id).map_err(EngineScheduleError::Pool)?;
                    return Err(EngineScheduleError::Submit(error));
                }
            };
            let next = self.perform(storage, &step)?;
            Ok((id, next))
        } else {
            let insertion = match self
                .scheduler
                .submit_idle(&mut items, &snapshot.busy, id, window)
            {
                Ok(insertion) => insertion,
                Err(error) => {
                    self.return_item(id).map_err(EngineScheduleError::Pool)?;
                    return Err(EngineScheduleError::Submit(error));
                }
            };
            self.hardware
                .start_live(&mut self.controller, storage, &items, insertion)
                .map_err(EngineScheduleError::Start)?;
            Ok((id, SchedulerNext::Finished))
        }
    }

    /// One finite observation and publication step; the caller retains the
    /// deadline and waits on the existing timer/IRQ wake between steps.
    pub fn advance<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        wait: SchedulerWait,
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerNext, EngineScheduleError<S::Error>> {
        let observation = self
            .hardware
            .observe_live(
                &mut self.controller,
                storage,
                wait,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(EngineScheduleError::Hardware)?;
        if matches!(
            observation,
            super::scheduler::SchedulerObservation::Skip(
                crate::bluetooth::BluetoothSchedulerSkipDisposition::UnsupportedHardwareResult
            )
        ) {
            // The vendor's unknown skip outcome withholds every detached item;
            // no later completion can turn that uncertainty into CPU custody.
            for id in self.scheduler.cancellation_items() {
                let result = match id.kind() {
                    SchedulerRoleKind::DirectTestMode => self.dtm.withhold(id),
                    SchedulerRoleKind::LegacyAdvertising => self.advertising.withhold(id),
                    SchedulerRoleKind::ConnectableAdvertising => self.connectable.withhold(id),
                    SchedulerRoleKind::LegacyScanning => self.scanning.withhold(id),
                    SchedulerRoleKind::PeripheralConnection => self.connections.withhold(id),
                };
                result.map_err(EngineScheduleError::Pool)?;
            }
        }
        let mut items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let step = match self.scheduler.advance(&mut items, observation) {
            Ok(step) => step,
            Err(fault) => {
                self.hardware
                    .recover_live(&mut self.controller, storage, fault);
                return Err(EngineScheduleError::Transaction(fault));
            }
        };
        self.perform(storage, &step)
    }

    pub fn cancel<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        ids: &[SchedulerItemId],
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerNext, EngineScheduleError<S::Error>> {
        let mut items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let snapshot = self
            .hardware
            .snapshot_live(
                &mut self.controller,
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(EngineScheduleError::Hardware)?;
        let step = self
            .scheduler
            .begin_cancel(&mut items, &snapshot.busy, snapshot.head, ids)
            .map_err(EngineScheduleError::Cancel)?;
        self.perform(storage, &step)
    }

    /// Return CPU custody after a fenced list observation and bounded scan.
    pub fn complete(
        &mut self,
        observed: crate::bluetooth::BluetoothSchedulerFinishedHardwareListObserved,
    ) -> Result<super::scheduler::SchedulerCompletion<SchedulerItemId, Q>, CompletionError> {
        if observed.index() != crate::bluetooth::BluetoothSchedulerHardwareListIndex::ZERO {
            return Err(CompletionError::WrongList(observed));
        }
        let mut items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let completion = match self.scheduler.take_completed(&mut items) {
            Ok(completion) => completion,
            Err(_) => return Err(CompletionError::TransactionActive(observed)),
        };
        for (id, _) in completion.iter() {
            self.return_item(id).map_err(CompletionError::Pool)?;
        }
        Ok(completion)
    }

    pub fn flush<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerNext, EngineScheduleError<S::Error>> {
        let items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let snapshot = self
            .hardware
            .snapshot_live(
                &mut self.controller,
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(EngineScheduleError::Hardware)?;
        let step = self
            .scheduler
            .begin_flush(&items, &snapshot.busy)
            .map_err(|_| EngineScheduleError::Cancel(SchedulerCancelError::TransactionActive))?;
        self.perform(storage, &step)
    }

    pub fn step_stop<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        stop: crate::bluetooth::BluetoothSchedulerStop,
        clock: &dyn oer_time::Clock,
    ) -> Result<crate::bluetooth::BluetoothSchedulerStopStep, super::scheduler::SchedulerStopError>
    {
        self.hardware.step_stop_live(
            &mut self.controller,
            storage,
            stop,
            Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
        )
    }

    pub fn enter_stopped(
        &mut self,
        stopped: crate::bluetooth::BluetoothSchedulerStopped,
    ) -> Result<(), super::scheduler::SchedulerStopRejected> {
        self.scheduler.enter_stopped(stopped)
    }

    pub fn stopped(&self) -> Option<&crate::bluetooth::BluetoothSchedulerStopped> {
        self.scheduler.stopped()
    }

    pub fn restart_if_idle<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        clock: &dyn oer_time::Clock,
    ) -> Result<(), EngineScheduleError<S::Error>> {
        let items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let snapshot = self
            .hardware
            .snapshot_live(
                &mut self.controller,
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(EngineScheduleError::Hardware)?;
        if let Some(insertion) = self.scheduler.restart_if_idle(&items, &snapshot.busy) {
            self.hardware
                .start_live(&mut self.controller, storage, &items, insertion)
                .map_err(EngineScheduleError::Start)?;
        }
        Ok(())
    }

    pub fn resume<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
    ) -> Result<(), EngineScheduleError<S::Error>> {
        let items = SchedulerItemSpace::new()
            .with(&self.dtm)
            .with(&self.advertising)
            .with(&self.connectable)
            .with(&self.scanning)
            .with(&self.connections);
        let insertion = self
            .scheduler
            .resume(&items)
            .map_err(|_| EngineScheduleError::NotStopped)?;
        if let Some(insertion) = insertion {
            self.hardware
                .start_live(&mut self.controller, storage, &items, insertion)
                .map_err(EngineScheduleError::Start)?;
        }
        Ok(())
    }
}
