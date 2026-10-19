//! Scheduler list zero and the global receive chains through the powered task
//! endpoint.
//!
//! Every address reaches hardware only after it resolves to a submitted item
//! of the caller's scheduler item space. Pools bind their storage for
//! `'static`, so a resolved item stays valid memory for as long as hardware
//! may follow it.

use oer_esp32s31_bluetooth_memory::{
    ControllerSramLinkAddress, LeDeviceTablePublication, LeRxChain, RxMemoryListClass,
    SchedulerItemSpace,
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    BluetoothSchedulerStopped,
};

use super::ControllerPoweredTaskRuntime;
use crate::{
    interrupt::SchedulerWakeBatch,
    scheduler::{
        SchedulerAction, SchedulerFinishedListCaptureError, SchedulerHardwareError,
        SchedulerHardwareView, SchedulerIdleInsertion, SchedulerObservation,
        SchedulerRunInterruptStorage, SchedulerStartError, SchedulerStep, SchedulerStopError,
        SchedulerTransactionFault, SchedulerWait, hardware::live::LiveSchedulerBackend,
    },
};

/// Why the global receive chains were not published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerRxChainError {
    /// A RUN command was already published in this epoch.
    SchedulerStarted,
    /// A chain belongs to the other class.
    WrongClass,
}

impl ControllerPoweredTaskRuntime<'_> {
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
        for action in step.actions() {
            let link = match action {
                SchedulerAction::PublishExecutionLock(link)
                | SchedulerAction::PublishLockModify(link)
                | SchedulerAction::PublishHead(Some(link))
                | SchedulerAction::PublishSkip(link) => link,
                _ => continue,
            };
            if items.listed_item(link).is_none() {
                return Err(SchedulerHardwareError::ForeignItem(link));
            }
        }
        let mut controller = self.task.controller();
        let mut backend = LiveSchedulerBackend {
            controller: &mut controller,
            storage,
        };
        self.runtime.scheduler_hardware.perform(&mut backend, step)
    }

    /// Take one fresh observation of the awaited publication.
    pub fn observe_scheduler(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError> {
        let mut controller = self.task.controller();
        let mut backend = LiveSchedulerBackend {
            controller: &mut controller,
            storage,
        };
        self.runtime.scheduler_hardware.observe(&mut backend, wait)
    }

    /// Clear what a faulted list transaction left published.
    pub fn recover_scheduler(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
        fault: SchedulerTransactionFault,
    ) {
        let mut controller = self.task.controller();
        let mut backend = LiveSchedulerBackend {
            controller: &mut controller,
            storage,
        };
        self.runtime.scheduler_hardware.recover(&mut backend, fault);
    }

    /// Publish the idle insertion's head and start the scheduler with RUN.
    pub fn start_scheduler<S: SchedulerRunInterruptStorage>(
        &mut self,
        storage: &S,
        items: &SchedulerItemSpace<'_>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<S::Error>> {
        if items.listed_item(insertion.head).is_none() {
            return Err(SchedulerStartError::ForeignItem(insertion.head));
        }
        let mut controller = self.task.controller();
        let mut backend = LiveSchedulerBackend {
            controller: &mut controller,
            storage,
        };
        self.runtime
            .scheduler_hardware
            .start(&mut backend, insertion)
    }

    /// Sample scheduler BUSY, then the head of list zero.
    pub fn observe_scheduler_hardware(
        &mut self,
        storage: &impl SchedulerRunInterruptStorage,
    ) -> Result<SchedulerHardwareView, SchedulerHardwareError> {
        let busy = storage
            .with_interrupt_registers((), |interrupts, ()| {
                interrupts.capture_scheduler_busy(crate::scheduler::DIAGNOSTIC_READ_BUDGET)
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
    ) -> Result<BluetoothSchedulerStopStep, SchedulerStopError> {
        let mut controller = self.task.controller();
        storage
            .with_interrupt_registers(stop, |interrupts, stop| {
                controller.step_scheduler_stop(
                    interrupts,
                    stop,
                    crate::scheduler::DIAGNOSTIC_READ_BUDGET,
                )
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
    /// # Safety
    ///
    /// The caller has reset the pinned scanner graph since the last scanner
    /// it accepted, and published the receive chains before the first RUN
    /// of this epoch.
    #[allow(
        unsafe_code,
        reason = "the caller publishes the scanner graph and the receive chains before a scanner starts"
    )]
    pub unsafe fn publish_scan_start(&mut self) {
        let mut controller = self.task.controller();
        // SAFETY: the caller reset the scanner graph and published the
        // receive chains (this function's contract), and the task endpoint,
        // borrowed mutably, serializes the scanner registers.
        let _published = unsafe { controller.publish_scan_start() };
    }

    /// Publish where the device table starts and how many entries it holds.
    #[allow(
        unsafe_code,
        reason = "the device table owner binds its storage in controller SRAM for 'static"
    )]
    pub fn publish_device_table(&mut self, publication: LeDeviceTablePublication) {
        let mut controller = self.task.controller();
        // SAFETY: the publication comes from a device table bound to 'static
        // controller SRAM, whose first `count` entries are written, and the
        // task endpoint serializes the device-table registers.
        let _published =
            unsafe { controller.publish_device_table(publication.first_entry, publication.count) };
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
    #[allow(
        unsafe_code,
        reason = "the chains bind 'static storage and no RUN was published in this epoch"
    )]
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
        // SAFETY: both chains bind 'static pinned storage, no RUN was
        // published in this powered epoch, and the task endpoint serializes
        // the list registers.
        let _scanning = unsafe {
            controller.publish_rx_memory_list_initial_head(
                scanning.class().selector(),
                scanning.initial_cursor(),
            )
        };
        // SAFETY: as above.
        let _non_scanning = unsafe {
            controller.publish_software_connection_rx_memory_list_initial_head(
                non_scanning.class().selector(),
                non_scanning.initial_cursor(),
            )
        };
        Ok(())
    }
}
