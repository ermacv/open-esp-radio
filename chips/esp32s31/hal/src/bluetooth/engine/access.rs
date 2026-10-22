//! CPU access to role graphs stays tied to the complete owning engine.

use super::*;

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
    pub fn dtm(&mut self) -> Result<&mut DtmPool<'pool, 'epoch, 1>, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.dtm)
    }
    pub fn advertising(
        &mut self,
    ) -> Result<&mut LegacyAdvertisingPool<'pool, 'epoch, A>, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.advertising)
    }
    pub fn connectable_advertising(
        &mut self,
    ) -> Result<&mut LegacyConnectableAdvertisingPool<'pool, 'epoch, C>, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.connectable)
    }
    pub fn scanning(&mut self) -> Result<&mut LegacyScanPool<'pool, 'epoch, 1>, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.scanning)
    }
    pub fn connections(
        &mut self,
    ) -> Result<&mut PeripheralConnectionPool<'pool, 'epoch, N>, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.connections)
    }
    pub fn devices(&mut self) -> Result<&mut LeDeviceTable, EngineFault> {
        if self.faulted {
            return Err(EngineFault);
        }
        Ok(&mut self.memory.devices)
    }
    pub fn receive(
        &mut self,
        source: LeRxSource,
    ) -> Result<Option<LeRxOutcome>, EngineReceiveError> {
        if self.faulted {
            return Err(EngineReceiveError::Quarantined);
        }
        match source.class() {
            RxMemoryListClass::Scanning => self.memory.scanning_rx.take(source),
            RxMemoryListClass::NonScanning => self.memory.other_rx.take(source),
        }
        .map_err(EngineReceiveError::Receive)
    }
    pub fn reset_scanning(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        image: ScanResetImage,
    ) -> Result<(), EnginePrepareError<LegacyScanError>> {
        if self.faulted {
            return Err(EnginePrepareError::Quarantined);
        }
        self.memory
            .scanning
            .reset(lease, &self.memory.scanning_rx, image)
            .map_err(EnginePrepareError::Role)
    }
    pub fn prepare_connectable_advertising(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        input: LegacyConnectableAdvertisingMemoryInput<'_>,
        power: LeTxPower,
        priority: SchedulerEventPriority,
    ) -> Result<(), EnginePrepareError<LegacyConnectableAdvertisingError>> {
        if self.faulted {
            return Err(EnginePrepareError::Quarantined);
        }
        self.memory
            .connectable
            .prepare(lease, input, &self.memory.other_rx, power, priority)
            .map_err(EnginePrepareError::Role)
    }
    pub fn prepare_first_connection(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        event: PeripheralConnectionFirstEvent,
    ) -> Result<PeripheralConnectionEvent, EnginePrepareError<PeripheralConnectionError>> {
        if self.faulted {
            return Err(EnginePrepareError::Quarantined);
        }
        let workspace = self.direction_finding_link();
        self.memory
            .connections
            .prepare_first_event(lease, event, workspace)
            .map_err(EnginePrepareError::Role)
    }
    pub fn publish_device_table(&mut self) -> Result<(), EngineFault> {
        use crate::bluetooth::ControllerHalBorrow;
        if self.faulted {
            return Err(EngineFault);
        }
        self.memory
            .devices
            .publish(&mut self.registers.borrow_bluetooth_controller());
        Ok(())
    }

    /// Model the stop receipt; production accepts it only from this engine's
    /// own finite hardware-stop protocol.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn stopped_for_host_test(&mut self) {
        let _ = self
            .memory
            .scheduler
            .enter_stopped(crate::bluetooth::BluetoothSchedulerStopped::for_host_test());
    }

    pub(super) fn return_item(&mut self, id: SchedulerItemId) -> Result<(), SchedulerPoolError> {
        match id.kind() {
            SchedulerRoleKind::DirectTestMode => self.memory.dtm.retire(id),
            SchedulerRoleKind::LegacyAdvertising => self.memory.advertising.retire(id),
            SchedulerRoleKind::ConnectableAdvertising => self.memory.connectable.retire(id),
            SchedulerRoleKind::LegacyScanning => self.memory.scanning.retire(id),
            SchedulerRoleKind::PeripheralConnection => self.memory.connections.retire(id),
        }
    }

    /// Return CPU custody after a fenced list observation and bounded scan.
    pub fn complete(
        &mut self,
        observed: EngineFinishedHardwareListObserved,
    ) -> Result<super::scheduler::SchedulerCompletion<SchedulerItemId, Q>, CompletionError> {
        if self.faulted {
            return Err(CompletionError::Quarantined(observed));
        }
        if observed.epoch != self.epoch {
            return Err(CompletionError::ForeignEpoch(observed));
        }
        if observed.index() != crate::bluetooth::BluetoothSchedulerHardwareListIndex::ZERO {
            return Err(CompletionError::WrongList(observed));
        }
        let mut items = SchedulerItemSpace::new()
            .with(&self.memory.dtm)
            .with(&self.memory.advertising)
            .with(&self.memory.connectable)
            .with(&self.memory.scanning)
            .with(&self.memory.connections);
        let completion = match self.memory.scheduler.take_completed(&mut items) {
            Ok(completion) => completion,
            Err(_) => return Err(CompletionError::TransactionActive(observed)),
        };
        for (id, _) in completion.iter() {
            self.return_item(id).map_err(CompletionError::Pool)?;
        }
        Ok(completion)
    }

    pub fn scheduled_count(&self) -> usize {
        self.memory.scheduler.list().len()
    }
}

#[derive(Debug)]
pub enum EnginePrepareError<E> {
    Quarantined,
    Role(E),
}
#[derive(Debug)]
pub enum EngineReceiveError {
    Quarantined,
    Receive(LeRxChainError),
}
