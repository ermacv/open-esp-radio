//! The hardware operations the runtime performs for the radio role.

use oer_esp32s31_bluetooth::{
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequest,
        ControllerTimeRequestError,
    },
    interrupt::SchedulerWakeBatch,
    scheduler::{
        SchedulerFinishedListCaptureError, SchedulerFinishedListWorkerStep, SchedulerHardwareError,
        SchedulerHardwareView, SchedulerIdleInsertion, SchedulerObservation,
        SchedulerSoftwareConfig, SchedulerStartError, SchedulerStep, SchedulerStopError,
        SchedulerTransactionFault, SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_memory::{LeDeviceTablePublication, LeRxChain, SchedulerItemSpace};
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerTimeScale, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
    BluetoothSchedulerStopped,
};

/// Why the global receive chains were not published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxChainPublicationError {
    /// A RUN command was already published in this epoch.
    SchedulerStarted,
    /// A chain belongs to the other class.
    WrongClass,
}

/// Every hardware operation of the Bluetooth radio runtime.
///
/// The live implementation joins the powered task endpoint of one Controller
/// epoch with the platform's interrupt-owner storage. Each method is one
/// finite operation; the runtime arranges every wait between them.
pub trait BluetoothRadioHardware {
    /// Why the platform could not prepare the scheduler-run interrupts.
    type StartError: Copy + core::fmt::Debug;

    /// Scheduler policy of this epoch.
    fn scheduler_config(&self) -> SchedulerSoftwareConfig;

    /// Controller time scale of this epoch.
    fn controller_time_scale(&self) -> BluetoothControllerTimeScale;

    /// Worst-case accuracy of the board's sleep clock, in parts per million.
    fn local_sleep_clock_ppm(&self) -> u16;

    /// Publish a controller-time latch request.
    fn request_time(&mut self) -> Result<ControllerTimeRequest, ControllerTimeRequestError>;

    /// Observe the latch request once.
    fn recheck_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimeEventStep, ControllerTimeEventError>;

    /// Drain a latch request a cancelled caller abandoned.
    fn drain_time(&mut self) -> Result<ControllerTimeEventStep, ControllerTimeEventError>;

    /// Publish the initial receive headers of both global chains.
    fn publish_rx_chains<const SCANNING: usize, const NON_SCANNING: usize>(
        &mut self,
        scanning: &LeRxChain<SCANNING>,
        non_scanning: &LeRxChain<NON_SCANNING>,
    ) -> Result<(), RxChainPublicationError>;

    /// Publish the scan-backoff state of a starting scanner.
    fn publish_scan_start(&mut self);

    /// Publish the device table after a change of the filter accept list.
    fn publish_device_table(&mut self, publication: LeDeviceTablePublication);

    /// Take the coalesced scheduler wake the interrupt published.
    fn take_wake(&mut self) -> Option<SchedulerWakeBatch>;

    /// Capture the finished lists a wake announced.
    fn capture_finished_lists(
        &mut self,
        wake: SchedulerWakeBatch,
    ) -> Result<(), SchedulerFinishedListCaptureError>;

    /// Capture the finished lists of a stopped scheduler.
    fn capture_stopped_finished_lists(
        &mut self,
        stopped: &BluetoothSchedulerStopped,
    ) -> Result<(), SchedulerFinishedListCaptureError>;

    /// Take one captured finished list.
    fn next_finished_list(&mut self) -> SchedulerFinishedListWorkerStep;

    /// Sample the scheduler work state, then the head of list zero.
    fn observe(&mut self) -> Result<SchedulerHardwareView, SchedulerHardwareError>;

    /// Owner of a disabled BLE PHY ETM route; only
    /// [`Self::restore_phy_etm_route`] consumes it.
    type DisabledPhyRoute;

    /// Disable the BLE PHY ETM route for a Direct Test Mode session.
    fn disable_phy_etm_route(&mut self) -> Self::DisabledPhyRoute;

    /// Route and enable the BLE PHY ETM channel again after a test.
    fn restore_phy_etm_route(&mut self, route: Self::DisabledPhyRoute);

    /// Publish the insertion's head and start the idle scheduler.
    fn start(
        &mut self,
        items: &SchedulerItemSpace<'_>,
        insertion: SchedulerIdleInsertion,
    ) -> Result<(), SchedulerStartError<Self::StartError>>;

    /// Perform the actions of one executor step.
    fn perform<I: Copy, const CAPACITY: usize>(
        &mut self,
        items: &SchedulerItemSpace<'_>,
        step: &SchedulerStep<I, CAPACITY>,
    ) -> Result<(), SchedulerHardwareError>;

    /// Take one fresh observation of the awaited publication.
    fn observe_wait(
        &mut self,
        wait: SchedulerWait,
    ) -> Result<SchedulerObservation, SchedulerHardwareError>;

    /// Clear what a faulted list transaction left published.
    fn recover(&mut self, fault: SchedulerTransactionFault);

    /// Advance the scheduler stop sequence by one step.
    fn step_stop(
        &mut self,
        stop: BluetoothSchedulerStop,
    ) -> Result<BluetoothSchedulerStopStep, SchedulerStopError>;
}

#[cfg(target_arch = "riscv32")]
mod live {
    use oer_esp32s31_bluetooth::{
        runtime_resources::{ControllerPoweredTaskRuntime, ControllerRxChainError},
        scheduler::SchedulerRunInterruptStorage,
    };

    use super::*;

    /// The powered task endpoint of one Controller epoch, the platform's
    /// interrupt-owner storage and the board's sleep-clock accuracy.
    pub struct LiveBluetoothHardware<'runtime, S> {
        task: ControllerPoweredTaskRuntime<'runtime>,
        storage: &'runtime S,
        local_sleep_clock_ppm: u16,
    }

    impl<'runtime, S: SchedulerRunInterruptStorage> LiveBluetoothHardware<'runtime, S> {
        /// Join the task endpoint with the storage that holds the stable
        /// interrupt owner. `local_sleep_clock_ppm` is the board's reviewed
        /// worst-case sleep-clock accuracy.
        pub const fn new(
            task: ControllerPoweredTaskRuntime<'runtime>,
            storage: &'runtime S,
            local_sleep_clock_ppm: u16,
        ) -> Self {
            Self {
                task,
                storage,
                local_sleep_clock_ppm,
            }
        }

        /// Separate the task endpoint again.
        pub fn into_task(self) -> ControllerPoweredTaskRuntime<'runtime> {
            self.task
        }
    }

    impl<S: SchedulerRunInterruptStorage> BluetoothRadioHardware for LiveBluetoothHardware<'_, S> {
        type StartError = S::Error;
        type DisabledPhyRoute = oer_esp32s31_hal::bluetooth::BlePhyEtmRouteDisabled;

        fn scheduler_config(&self) -> SchedulerSoftwareConfig {
            self.task.scheduler_config()
        }

        fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
            self.task.controller_time_scale()
        }

        fn local_sleep_clock_ppm(&self) -> u16 {
            self.local_sleep_clock_ppm
        }

        fn request_time(&mut self) -> Result<ControllerTimeRequest, ControllerTimeRequestError> {
            self.task.request_controller_time()
        }

        fn recheck_time(
            &mut self,
            request: ControllerTimeRequest,
        ) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
            self.task.recheck_owned_controller_time(request)
        }

        fn drain_time(&mut self) -> Result<ControllerTimeEventStep, ControllerTimeEventError> {
            self.task.drain_orphan_controller_time()
        }

        fn publish_rx_chains<const SCANNING: usize, const NON_SCANNING: usize>(
            &mut self,
            scanning: &LeRxChain<SCANNING>,
            non_scanning: &LeRxChain<NON_SCANNING>,
        ) -> Result<(), RxChainPublicationError> {
            self.task
                .publish_rx_chains(scanning, non_scanning)
                .map_err(|error| match error {
                    ControllerRxChainError::SchedulerStarted => {
                        RxChainPublicationError::SchedulerStarted
                    }
                    ControllerRxChainError::WrongClass => RxChainPublicationError::WrongClass,
                })
        }

        fn publish_scan_start(&mut self) {
            self.task.publish_scan_start();
        }

        fn publish_device_table(&mut self, publication: LeDeviceTablePublication) {
            self.task.publish_device_table(publication);
        }

        fn take_wake(&mut self) -> Option<SchedulerWakeBatch> {
            self.task.scheduler_wake().take()
        }

        fn capture_finished_lists(
            &mut self,
            wake: SchedulerWakeBatch,
        ) -> Result<(), SchedulerFinishedListCaptureError> {
            self.task.capture_scheduler_finished_lists(wake)
        }

        fn capture_stopped_finished_lists(
            &mut self,
            stopped: &BluetoothSchedulerStopped,
        ) -> Result<(), SchedulerFinishedListCaptureError> {
            self.task.capture_stopped_scheduler_finished_lists(stopped)
        }

        fn next_finished_list(&mut self) -> SchedulerFinishedListWorkerStep {
            self.task.scheduler_finished_lists().step()
        }

        fn observe(&mut self) -> Result<SchedulerHardwareView, SchedulerHardwareError> {
            self.task.observe_scheduler_hardware(self.storage)
        }

        fn disable_phy_etm_route(&mut self) -> Self::DisabledPhyRoute {
            self.task.disable_ble_phy_etm_route()
        }

        fn restore_phy_etm_route(&mut self, route: Self::DisabledPhyRoute) {
            self.task.restore_ble_phy_etm_route(route);
        }

        fn start(
            &mut self,
            items: &SchedulerItemSpace<'_>,
            insertion: SchedulerIdleInsertion,
        ) -> Result<(), SchedulerStartError<S::Error>> {
            self.task.start_scheduler(self.storage, items, insertion)
        }

        fn perform<I: Copy, const CAPACITY: usize>(
            &mut self,
            items: &SchedulerItemSpace<'_>,
            step: &SchedulerStep<I, CAPACITY>,
        ) -> Result<(), SchedulerHardwareError> {
            self.task.perform_scheduler_step(self.storage, items, step)
        }

        fn observe_wait(
            &mut self,
            wait: SchedulerWait,
        ) -> Result<SchedulerObservation, SchedulerHardwareError> {
            self.task.observe_scheduler(self.storage, wait)
        }

        fn recover(&mut self, fault: SchedulerTransactionFault) {
            self.task.recover_scheduler(self.storage, fault);
        }

        fn step_stop(
            &mut self,
            stop: BluetoothSchedulerStop,
        ) -> Result<BluetoothSchedulerStopStep, SchedulerStopError> {
            self.task.step_scheduler_stop(self.storage, stop)
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub use live::LiveBluetoothHardware;
