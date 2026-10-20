//! Scheduler list zero and the global receive chains through the powered task
//! endpoint.
//!
//! Every address reaches hardware only after it resolves to a submitted item
//! of the caller's scheduler item space. Pools bind their storage for
//! `'static`, so a resolved item stays valid memory for as long as hardware
//! may follow it.

use oer_esp32s31_bluetooth_memory::{
    ControllerSramLinkAddress, LeRxChain, RxMemoryListClass, SchedulerItemSpace,
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    BluetoothSchedulerStopped, DeviceTablePublication,
};
use oer_interrupt_table::signal::SignalExecutor;

use super::ControllerPoweredTaskRuntime;
use crate::{interrupt::SchedulerWakeBatch, scheduler::SchedulerFinishedListCaptureError};
use oer_esp32s31_bluetooth_memory::scheduler::{
    SchedulerHardwareError, SchedulerHardwareView, SchedulerIdleInsertion, SchedulerObservation,
    SchedulerRunInterruptStorage, SchedulerStartError, SchedulerStep, SchedulerStopError,
    SchedulerTransactionFault, SchedulerWait,
};

/// Why the scanner start was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerScanStartError {
    /// The receive chains were not published in this epoch.
    RxChainsNotPublished,
    /// The HAL refused the scanning list's publication.
    Refused(oer_esp32s31_hal::bluetooth::ScanStartError),
}

/// Why the global receive chains were not published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRxChainError {
    /// A RUN command was already published in this epoch.
    SchedulerStarted,
    /// A chain belongs to the other class.
    WrongClass,
}

impl<X: SignalExecutor> ControllerPoweredTaskRuntime<'_, X> {
    /// Perform the actions of one executor step on list zero, in order.
    ///
    /// Every named item must be submitted in `items`; otherwise nothing is
    /// performed.
    pub fn perform_scheduler_step<I: Copy, const CAPACITY: usize>(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        items: &SchedulerItemSpace<'_>,
        step: &SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError> {
        let mut controller = self.task.controller();
        self.runtime
            .scheduler_hardware
            .perform_live(&mut controller, storage, items, step)
    }

    /// Take one fresh observation of the awaited publication; each
    /// diagnostic sample settles on `clock`.
    pub fn observe_scheduler(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        wait: SchedulerWait,
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        let mut controller = self.task.controller();
        self.runtime.scheduler_hardware.observe_live(
            &mut controller,
            storage,
            wait,
            crate::diagnostic_settle(clock),
        )
    }

    /// Clear what a faulted list transaction left published.
    pub fn recover_scheduler(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        fault: SchedulerTransactionFault,
    ) {
        let mut controller = self.task.controller();
        self.runtime
            .scheduler_hardware
            .recover_live(&mut controller, storage, fault);
    }

    /// Publish the idle insertion's head and start the scheduler with RUN.
    pub fn start_scheduler<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        items: &SchedulerItemSpace<'_>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<S::Error>> {
        let mut controller = self.task.controller();
        self.runtime
            .scheduler_hardware
            .start_live(&mut controller, storage, items, insertion)
    }

    /// Sample scheduler BUSY, then the head of list zero.
    pub fn observe_scheduler_hardware(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        clock: &dyn oer_time::Clock,
    ) -> Result<SchedulerHardwareView, SchedulerHardwareError> {
        let busy = storage
            .with_interrupt_registers((), |interrupts, ()| {
                interrupts.capture_scheduler_busy(crate::diagnostic_settle(clock))
            })
            .map_err(|()| SchedulerHardwareError::InterruptOwnerUnavailable)?
            .map_err(|_| SchedulerHardwareError::DiagnosticUnsettled)?;
        let head = self
            .task
            .controller()
            .observe_scheduler_hardware_list_head(BluetoothSchedulerHardwareListIndex::ZERO);
        let hardware_head = match head.address() {
            None => None,
            Some(address) => Some(
                ControllerSramLinkAddress::new(address.address())
                    .map_err(|_| SchedulerHardwareError::ForeignHead)?,
            ),
        };
        Ok(SchedulerHardwareView {
            busy,
            hardware_head,
        })
    }

    /// Capture one fenced finished-list transfer into this epoch's worker.
    pub fn capture_scheduler_finished_lists(
        &mut self,
        wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        let mut controller = self.task.controller();
        self.runtime
            .scheduler_finished_lists
            .capture(&mut controller, wake)
    }

    /// Advance the common scheduler stop sequence by one finite step,
    /// serialized with the interrupt owner.
    ///
    /// Storage without the interrupt owner, or a BUSY sample that never
    /// settled, ends the sequence with its typed reason.
    pub fn step_scheduler_stop(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        stop: BluetoothSchedulerStop,
        clock: &dyn oer_time::Clock,
    ) -> Result<BluetoothSchedulerStopStep, SchedulerStopError> {
        let mut controller = self.task.controller();
        storage
            .with_interrupt_registers(stop, |interrupts, stop| {
                controller.step_scheduler_stop(interrupts, stop, crate::diagnostic_settle(clock))
            })
            .map_err(|_| SchedulerStopError::InterruptOwnerUnavailable)?
            .map_err(|_| SchedulerStopError::DiagnosticUnsettled)
    }

    /// Disable the BLE PHY ETM route for one Direct Test Mode event.
    pub fn disable_ble_phy_etm_route(
        &mut self,
    ) -> oer_esp32s31_hal::bluetooth::BlePhyEtmRouteDisabled {
        self.task.controller().disable_ble_phy_etm_route()
    }

    /// Publish the scanner start's scan-backoff state: both backoff words
    /// start at one under the S31 configuration's maximum upper limit, as the pinned
    /// `r_ble_lll_scan_start` does after it resets the scanner link state.
    ///
    /// The caller has reset the pinned scanner graph since the last scanner
    /// it accepted. The scanner writes through the scanning RX list, so the
    /// start takes that list's publication of this epoch.
    ///
    /// # Errors
    ///
    /// The receive chains were not published in this epoch, or the HAL
    /// refused the publication; nothing is written.
    pub fn publish_scan_start(&mut self) -> Result<(), ControllerScanStartError> {
        let scanning = self
            .scanning_rx
            .as_ref()
            .ok_or(ControllerScanStartError::RxChainsNotPublished)?;
        let mut controller = self.task.controller();
        controller
            .publish_scan_start(scanning)
            .map(|_published| ())
            .map_err(ControllerScanStartError::Refused)
    }

    /// Publish where the device table starts and how many entries it holds.
    pub fn publish_device_table(&mut self, publication: &DeviceTablePublication) {
        let mut controller = self.task.controller();
        let _published = controller.publish_device_table(publication);
    }

    /// Route and enable the BLE PHY ETM channel again after a test.
    pub fn restore_ble_phy_etm_route(
        &mut self,
        route: oer_esp32s31_hal::bluetooth::BlePhyEtmRouteDisabled,
    ) {
        self.task.controller().restore_ble_phy_etm_route(route);
    }

    /// Capture the finished lists of a stopped scheduler into this epoch's
    /// worker.
    pub fn capture_stopped_scheduler_finished_lists(
        &mut self,
        stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError> {
        let mut controller = self.task.controller();
        self.runtime
            .scheduler_finished_lists
            .capture_stopped(&mut controller, stopped)
    }

    /// Publish the initial receive headers of both global chains.
    ///
    /// The scanning chain restores the controller-default opcode policy; the
    /// non-scanning chain then installs the software-connection policy, which
    /// stays in force for every later event. Publication precedes the first
    /// RUN of the epoch.
    pub fn publish_rx_chains<const SCANNING: usize, const NON_SCANNING: usize>(
        &mut self,
        scanning: &LeRxChain<SCANNING>,
        non_scanning: &LeRxChain<NON_SCANNING>,
    ) -> Result<(), ControllerRxChainError> {
        if self.runtime.scheduler_hardware.has_run() {
            return Err(ControllerRxChainError::SchedulerStarted);
        }
        if scanning.class() != RxMemoryListClass::Scanning
            || non_scanning.class() != RxMemoryListClass::NonScanning
        {
            return Err(ControllerRxChainError::WrongClass);
        }
        let mut controller = self.task.controller();
        let scanning = controller.publish_rx_memory_list_initial_head(&scanning.publication_head());
        let _non_scanning = controller.publish_software_connection_rx_memory_list_initial_head(
            &non_scanning.publication_head(),
        );
        self.scanning_rx = Some(scanning);
        Ok(())
    }
}
