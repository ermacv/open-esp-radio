//! Address publication and completion return by the arena's single owner.

use super::scheduler::{
    SchedulerCancelError, SchedulerHardwareError, SchedulerNext, SchedulerRunInterruptStorage,
    SchedulerStartError, SchedulerStep, SchedulerSubmitError, SchedulerTransactionFault,
    SchedulerWait,
};
use super::*;
use crate::bluetooth::ControllerHalBorrow;
use crate::settle::Settle;

#[derive(Debug)]
pub enum EngineScheduleError<E> {
    Quarantined,
    NotInitialized,
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
    Quarantined,
    NotInitialized,
    SchedulerStarted,
    NotPublished,
    Scan(crate::bluetooth::ScanStartError),
}

impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> Engine<ArenaMemory<'pool, 'epoch, A, C, N, R, O, Q>>
{
    pub fn publish_receive_chains(&mut self) -> Result<(), ReceivePublicationError> {
        if self.faulted {
            return Err(ReceivePublicationError::Quarantined);
        }
        if !self.phy_initialized {
            return Err(ReceivePublicationError::NotInitialized);
        }
        if self.memory.hardware.has_run() {
            return Err(ReceivePublicationError::SchedulerStarted);
        }
        self.memory.scanning_publication = Some(
            self.registers
                .borrow_bluetooth_controller()
                .publish_rx_memory_list_initial_head(&self.memory.scanning_rx.publication_head()),
        );
        let _ = self
            .registers
            .borrow_bluetooth_controller()
            .publish_software_connection_rx_memory_list_initial_head(
                &self.memory.other_rx.publication_head(),
            );
        Ok(())
    }

    pub fn publish_scan_start(&mut self) -> Result<(), ReceivePublicationError> {
        if self.faulted {
            return Err(ReceivePublicationError::Quarantined);
        }
        let scanning = self
            .memory
            .scanning_publication
            .as_ref()
            .ok_or(ReceivePublicationError::NotPublished)?;
        self.registers
            .borrow_bluetooth_controller()
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
            SchedulerRoleKind::DirectTestMode => self.memory.dtm.submit(instance, item),
            SchedulerRoleKind::LegacyAdvertising => self.memory.advertising.submit(instance, item),
            SchedulerRoleKind::ConnectableAdvertising => {
                self.memory.connectable.submit(instance, item)
            }
            SchedulerRoleKind::LegacyScanning => self.memory.scanning.submit(instance, item),
            SchedulerRoleKind::PeripheralConnection => {
                self.memory.connections.submit(instance, item)
            }
        }
    }

    fn perform<E>(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        step: &SchedulerStep<SchedulerItemId, Q>,
    ) -> Result<SchedulerNext, EngineScheduleError<E>> {
        let items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        self.memory
            .hardware
            .perform_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                &items,
                step,
            )
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
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
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        if !self.phy_initialized {
            return Err(EngineScheduleError::NotInitialized);
        }
        let window = match instance.kind() {
            SchedulerRoleKind::DirectTestMode => self.memory.dtm.prepared_window(instance, item),
            SchedulerRoleKind::LegacyAdvertising => {
                self.memory.advertising.prepared_window(instance, item)
            }
            SchedulerRoleKind::ConnectableAdvertising => {
                self.memory.connectable.prepared_window(instance, item)
            }
            SchedulerRoleKind::LegacyScanning => {
                self.memory.scanning.prepared_window(instance, item)
            }
            SchedulerRoleKind::PeripheralConnection => {
                self.memory.connections.prepared_window(instance, item)
            }
        }
        .map_err(EngineScheduleError::Pool)?;
        let id = self
            .list_item(instance, item)
            .map_err(EngineScheduleError::Pool)?;
        let mut items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let snapshot = match self.memory.hardware.snapshot_live(
            &mut self.registers.borrow_bluetooth_controller(),
            storage,
            &items,
            Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.return_item(id).map_err(EngineScheduleError::Pool)?;
                self.faulted = true;
                return Err(EngineScheduleError::Hardware(error));
            }
        };
        if snapshot.busy.is_busy() {
            let step =
                match self
                    .memory
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
            let insertion =
                match self
                    .memory
                    .scheduler
                    .submit_idle(&mut items, &snapshot.busy, id, window)
                {
                    Ok(insertion) => insertion,
                    Err(error) => {
                        self.return_item(id).map_err(EngineScheduleError::Pool)?;
                        return Err(EngineScheduleError::Submit(error));
                    }
                };
            self.memory
                .hardware
                .start_live(
                    &mut self.registers.borrow_bluetooth_controller(),
                    storage,
                    &items,
                    insertion,
                )
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
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
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        let observation = self
            .memory
            .hardware
            .observe_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                wait,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        if matches!(
            observation,
            super::scheduler::SchedulerObservation::Skip(
                crate::bluetooth::BluetoothSchedulerSkipDisposition::UnsupportedHardwareResult
            )
        ) {
            // The vendor's unknown skip outcome withholds every detached item;
            // no later completion can turn that uncertainty into CPU custody.
            for id in self.memory.scheduler.cancellation_items() {
                let result = match id.kind() {
                    SchedulerRoleKind::DirectTestMode => self.memory.dtm.withhold(id),
                    SchedulerRoleKind::LegacyAdvertising => self.memory.advertising.withhold(id),
                    SchedulerRoleKind::ConnectableAdvertising => {
                        self.memory.connectable.withhold(id)
                    }
                    SchedulerRoleKind::LegacyScanning => self.memory.scanning.withhold(id),
                    SchedulerRoleKind::PeripheralConnection => self.memory.connections.withhold(id),
                };
                result.map_err(EngineScheduleError::Pool)?;
            }
            self.faulted = true;
            return Err(EngineScheduleError::Quarantined);
        }
        let mut items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let step = match self.memory.scheduler.advance(&mut items, observation) {
            Ok(step) => step,
            Err(fault) => {
                self.faulted = true;
                self.memory.hardware.recover_live(
                    &mut self.registers.borrow_bluetooth_controller(),
                    storage,
                    fault,
                );
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
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        let mut items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let snapshot = self
            .memory
            .hardware
            .snapshot_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        let step = self
            .memory
            .scheduler
            .begin_cancel(&mut items, &snapshot.busy, snapshot.head, ids)
            .map_err(EngineScheduleError::Cancel)?;
        self.perform(storage, &step)
    }

    pub fn flush<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerNext, EngineScheduleError<S::Error>> {
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        let items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let snapshot = self
            .memory
            .hardware
            .snapshot_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        let step = self
            .memory
            .scheduler
            .begin_flush(&items, &snapshot.busy)
            .map_err(|_| EngineScheduleError::Cancel(SchedulerCancelError::TransactionActive))?;
        self.perform(storage, &step)
    }

    /// Keep the stop receipt inside the engine that produced it.
    pub fn step_stop<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        stop: crate::bluetooth::BluetoothSchedulerStop,
        clock: &dyn oer_time::Clock,
    ) -> Result<EngineStopStep, EngineStopError> {
        if self.faulted {
            return Err(EngineStopError::Quarantined);
        }
        let result = self
            .memory
            .hardware
            .step_stop_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                stop,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(|error| {
                self.faulted = true;
                EngineStopError::Hardware(error)
            })?;
        match result {
            crate::bluetooth::BluetoothSchedulerStopStep::Pending(stop) => {
                Ok(EngineStopStep::Pending(stop))
            }
            crate::bluetooth::BluetoothSchedulerStopStep::Stopped(stopped) => {
                self.memory
                    .scheduler
                    .enter_stopped(stopped)
                    .map_err(EngineStopError::Rejected)?;
                Ok(EngineStopStep::Stopped)
            }
        }
    }

    pub fn restart_if_idle<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        clock: &dyn oer_time::Clock,
    ) -> Result<(), EngineScheduleError<S::Error>> {
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        let items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let snapshot = self
            .memory
            .hardware
            .snapshot_live(
                &mut self.registers.borrow_bluetooth_controller(),
                storage,
                &items,
                Settle::new(super::scheduler::DIAGNOSTIC_SETTLE, clock),
            )
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        if let Some(insertion) = self
            .memory
            .scheduler
            .restart_if_idle(&items, &snapshot.busy)
        {
            self.memory
                .hardware
                .start_live(
                    &mut self.registers.borrow_bluetooth_controller(),
                    storage,
                    &items,
                    insertion,
                )
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
        }
        Ok(())
    }

    pub fn resume<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
    ) -> Result<(), EngineScheduleError<S::Error>> {
        if self.faulted {
            return Err(EngineScheduleError::Quarantined);
        }
        let items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let insertion = self
            .memory
            .scheduler
            .resume(&items)
            .map_err(|_| EngineScheduleError::NotStopped)?;
        if let Some(insertion) = insertion {
            self.memory
                .hardware
                .start_live(
                    &mut self.registers.borrow_bluetooth_controller(),
                    storage,
                    &items,
                    insertion,
                )
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum EngineStopStep {
    Pending(crate::bluetooth::BluetoothSchedulerStop),
    Stopped,
}
#[derive(Debug)]
pub enum EngineStopError {
    Quarantined,
    Hardware(super::scheduler::SchedulerStopError),
    Rejected(super::scheduler::SchedulerStopRejected),
}
