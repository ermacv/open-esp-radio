//! Address publication and completion return by the arena's single owner.
//!
//! Scheduler operations await typed P3 replies without lending registers.
//! The caller bounds the whole operation with its deadline. Dropping an
//! exchange while it awaits a reply quarantines this engine's publications;
//! its static request slot and complete arena remain retained.

use super::scheduler::interrupt::{SchedulerDoorbell, SchedulerInterruptError};
use super::scheduler::{
    SchedulerCancelError, SchedulerHardwareError, SchedulerNext, SchedulerStartError,
    SchedulerStep, SchedulerSubmitError, SchedulerTransactionFault, SchedulerWait,
};
use super::*;
use crate::bluetooth::ControllerHalBorrow;

#[derive(Debug)]
pub enum EngineScheduleError {
    Quarantined,
    NotInitialized,
    Pool(SchedulerPoolError),
    Submit(SchedulerSubmitError<SchedulerItemId>),
    Cancel(SchedulerCancelError<SchedulerItemId>),
    Transaction(SchedulerTransactionFault),
    Hardware(SchedulerHardwareError),
    Start(SchedulerStartError<SchedulerInterruptError>),
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
> Engine<ArenaMemory<'pool, 'epoch, A, C, N, 1, R, O, Q>>
{
    pub fn publish_receive_chains(&mut self) -> Result<(), ReceivePublicationError> {
        if self.is_quarantined() {
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
                .publish_rx_memory_list_initial_head(
                    &self.memory.scanning_rx[0].publication_head(),
                ),
        );
        let _ = self
            .registers
            .borrow_bluetooth_controller()
            .publish_software_connection_rx_memory_list_initial_head(
                &self.memory.other_rx[0].publication_head(),
            );
        Ok(())
    }

    pub fn publish_scan_start(&mut self) -> Result<(), ReceivePublicationError> {
        if self.is_quarantined() {
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
}

impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> Engine<ArenaMemory<'pool, 'epoch, A, C, N, S, R, O, Q>>
{
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

    async fn perform(
        &mut self,
        storage: &impl SchedulerDoorbell,
        step: &SchedulerStep<SchedulerItemId, Q>,
    ) -> Result<SchedulerNext, EngineScheduleError> {
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
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                &items,
                step,
            )
            .await
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
    pub async fn submit<D: SchedulerDoorbell>(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        item: usize,
        storage: &D,
    ) -> Result<(SchedulerItemId, SchedulerNext), EngineScheduleError> {
        self.submit_with_live_insertion::<D, true>(instance, item, storage)
            .await
    }

    async fn submit_with_live_insertion<D: SchedulerDoorbell, const LIVE: bool>(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        item: usize,
        storage: &D,
    ) -> Result<(SchedulerItemId, SchedulerNext), EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
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
        let snapshot = match self
            .memory
            .hardware
            .snapshot_live(
                &mut self.registers.borrow_bluetooth_controller(),
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                &items,
            )
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.return_item(id).map_err(EngineScheduleError::Pool)?;
                self.faulted = true;
                return Err(EngineScheduleError::Hardware(error));
            }
        };
        if snapshot.busy.is_busy() {
            if !LIVE {
                self.allow_busy_insertion::<LIVE>(id)
                    .map_err(EngineScheduleError::Pool)?;
                return Err(EngineScheduleError::Submit(
                    SchedulerSubmitError::SchedulerBusy,
                ));
            }
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
            let next = self.perform(storage, &step).await?;
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
                    self.scheduler_interrupts
                        .as_mut()
                        .ok_or(EngineScheduleError::Hardware(
                            SchedulerHardwareError::Interrupt(
                                SchedulerInterruptError::NotInstalled,
                            ),
                        ))?,
                    storage,
                    &items,
                    insertion,
                )
                .await
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
            Ok((id, SchedulerNext::Finished))
        }
    }

    /// One finite observation and publication step; the caller retains the
    /// deadline and waits on the existing timer/IRQ wake between steps.
    pub async fn advance<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
        wait: SchedulerWait,
    ) -> Result<SchedulerNext, EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
        }
        let observation = self
            .memory
            .hardware
            .observe_live(
                &mut self.registers.borrow_bluetooth_controller(),
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                wait,
            )
            .await
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
                    self.scheduler_interrupts
                        .as_mut()
                        .ok_or(EngineScheduleError::Hardware(
                            SchedulerHardwareError::Interrupt(
                                SchedulerInterruptError::NotInstalled,
                            ),
                        ))?,
                    storage,
                    fault,
                );
                return Err(EngineScheduleError::Transaction(fault));
            }
        };
        self.perform(storage, &step).await
    }

    pub async fn cancel<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
        ids: &[SchedulerItemId],
    ) -> Result<SchedulerNext, EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
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
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                &items,
            )
            .await
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        let step = self
            .memory
            .scheduler
            .begin_cancel(&mut items, &snapshot.busy, snapshot.head, ids)
            .map_err(EngineScheduleError::Cancel)?;
        self.perform(storage, &step).await
    }

    pub async fn flush<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
    ) -> Result<SchedulerNext, EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
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
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                &items,
            )
            .await
            .map_err(|error| {
                self.faulted = true;
                EngineScheduleError::Hardware(error)
            })?;
        let step = self
            .memory
            .scheduler
            .begin_flush(&items, &snapshot.busy)
            .map_err(|_| EngineScheduleError::Cancel(SchedulerCancelError::TransactionActive))?;
        self.perform(storage, &step).await
    }

    /// Keep the stop receipt inside the engine that produced it.
    /// Poll the retained stop transaction; pending P3 commands stay in its
    /// channel and `stop`, independent of the lifetime of this borrow.
    #[inline(never)]
    pub fn poll_stop<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
        stop: &mut crate::bluetooth::BluetoothSchedulerStop,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Result<bool, EngineStopError>> {
        if self.is_quarantined() {
            return core::task::Poll::Ready(Err(EngineStopError::Quarantined));
        }
        let interrupts = self
            .scheduler_interrupts
            .as_mut()
            .ok_or(EngineStopError::Hardware(
                super::scheduler::SchedulerStopError::Interrupt(
                    SchedulerInterruptError::NotInstalled,
                ),
            ))?;
        let result = core::task::ready!(
            self.registers
                .borrow_bluetooth_controller()
                .poll_scheduler_stop(interrupts, stop, storage, cx)
        );
        core::task::Poll::Ready(match result {
            Err(error) => {
                self.faulted = true;
                Err(EngineStopError::Hardware(
                    super::scheduler::SchedulerStopError::Interrupt(error),
                ))
            }
            Ok(crate::bluetooth::scheduler_stop::Progress::Pending) => Ok(false),
            Ok(crate::bluetooth::scheduler_stop::Progress::Stopped(stopped)) => {
                self.memory
                    .scheduler
                    .enter_stopped(stopped)
                    .map_err(EngineStopError::Rejected)?;
                Ok(true)
            }
        })
    }

    pub async fn restart_if_idle<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
    ) -> Result<(), EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
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
                self.scheduler_interrupts
                    .as_mut()
                    .ok_or(EngineScheduleError::Hardware(
                        SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
                    ))?,
                storage,
                &items,
            )
            .await
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
                    self.scheduler_interrupts
                        .as_mut()
                        .ok_or(EngineScheduleError::Hardware(
                            SchedulerHardwareError::Interrupt(
                                SchedulerInterruptError::NotInstalled,
                            ),
                        ))?,
                    storage,
                    &items,
                    insertion,
                )
                .await
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
        }
        Ok(())
    }

    pub async fn resume<D: SchedulerDoorbell>(
        &mut self,
        storage: &D,
    ) -> Result<(), EngineScheduleError> {
        if self.is_quarantined() {
            return Err(EngineScheduleError::Quarantined);
        }
        if self.scheduler_interrupts.is_none() {
            return Err(EngineScheduleError::Hardware(
                SchedulerHardwareError::Interrupt(SchedulerInterruptError::NotInstalled),
            ));
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
                    self.scheduler_interrupts
                        .as_mut()
                        .ok_or(EngineScheduleError::Hardware(
                            SchedulerHardwareError::Interrupt(
                                SchedulerInterruptError::NotInstalled,
                            ),
                        ))?,
                    storage,
                    &items,
                    insertion,
                )
                .await
                .map_err(|error| {
                    self.faulted = true;
                    EngineScheduleError::Start(error)
                })?;
        }
        Ok(())
    }
}

impl<'pool, 'epoch, const Q: usize> Engine<ArenaMemory<'pool, 'epoch, 0, 0, 0, 0, 0, 0, Q>> {
    /// Publish the DTM profile's single head after the caller's stop/resume.
    /// A fresh P3 snapshot must prove idle. Busy returns the item to its pool
    /// and refuses publication without starting a lock/modify transaction.
    /// The DTM-only image therefore does not link live multi-role insertion.
    pub async fn submit_dtm<D: SchedulerDoorbell>(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        storage: &D,
    ) -> Result<(SchedulerItemId, SchedulerNext), EngineScheduleError> {
        self.submit_with_live_insertion::<D, false>(instance, 0, storage)
            .await
    }
}

#[derive(Debug)]
pub enum EngineStopError {
    Quarantined,
    Hardware(super::scheduler::SchedulerStopError),
    Rejected(super::scheduler::SchedulerStopRejected),
}
