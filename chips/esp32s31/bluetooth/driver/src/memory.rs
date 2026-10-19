//! Controller-SRAM storage, role instance pools and completion values.
//! Definitions live in the dedicated Bluetooth memory crate; these exports
//! expose only the memory contracts used by this backend.

pub use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineBindError, BlePhyEngineBindFailure, BlePhyEngineCpuOwned, BlePhyEngineStorage,
    ControllerSramLinkAddress, ControllerSramLinkAddressError, DtmPool, LeRxChain,
    LegacyAdvertisingPool, LegacyConnectableAdvertisingPool, LegacyScanPool,
    PeripheralConnectionPool, RxMemoryListClass, SchedulerAllocationConfig,
    SchedulerItemCompletionStatus, SchedulerItemId, SchedulerItemSpace, SchedulerRoleInstance,
};
