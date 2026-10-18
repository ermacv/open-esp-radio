#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! ESP32-S31 Bluetooth LE radio composition over Embassy-compatible time.
//!
//! The Bluetooth Controller is a client of the shared radio
//! (`oer-esp32s31-radio-runtime`). [`BluetoothParked::new`] claims the static
//! controller memory once per boot: the BLE PHY environment, the
//! direction-finding workspace, one pool per role and both global receive
//! chains, all placed in internal SRAM. [`start`] then runs one Controller
//! epoch from the parked partition to an installed radio runtime:
//!
//! 1. enters common radio power and enables the Bluetooth module clocks,
//!    Controller resets and low-power timer clock;
//! 2. runs Controller HAL, scheduler and modem low-power initialization;
//! 3. prepares the shared PHY through the radio system, joins it and the
//!    shared BTBB baseband, tracking the PHY when the join makes it due;
//! 4. enables the BLE base stack and publishes the public address;
//! 5. prepares Controller output, starts the runtime timer, publishes both
//!    interrupt owners and binds the three CPU routes to one dispatcher;
//! 6. installs the radio role in the runtime.
//!
//! It returns the running client and the port's backend
//! ([`BluetoothSystemBackend`]) for the body that runs the port.
//!
//! The dispatcher services source 124, 127 and 133 and forwards the scheduler
//! and source-127 worker wakes. The radio's driver runs inside its port,
//! joined with the HCI service; [`BluetoothSystem::run`] drives the
//! source-127 timer task until it faults or an interrupt service fails, and
//! publishes the Controller's active roles as Bluetooth LE status bits in the
//! radio system's coexistence schedule. Each epoch enables coexistence for
//! Bluetooth when it starts and disables it when it stops, after withdrawing
//! every status bit.
//! [`BluetoothSystem::stop`] takes what the port's uninstall returned
//! ([`BluetoothSystemUninstalled`]: the radio and hardware), reverses the
//! epoch and returns the parked partition with the controller memory back at its allocation-time image,
//! ready for the next [`start`]. Periodic PHY tracking belongs to the radio
//! system (`RadioSystem::run_tracking`).
//!
//! [`start_bluetooth_hci`] creates the HCI Controller once per boot: the
//! in-process transport, whose Host end goes to the Host stack, and the
//! service that runs the portable Controller core as the body of each
//! Controller epoch's port. Between Controller epochs the service retires the
//! drained Host end and restarts it with a fresh core.
//! [`BluetoothEntropy`] binds the SoC entropy service as its random source.
//!
//! Power and clock failures roll back to the parked client; any failure
//! after the first Controller write keeps its owners fail-stop.

#[cfg(any(target_arch = "riscv32", test))]
mod coex;
#[cfg(target_arch = "riscv32")]
mod hci;
#[cfg(target_arch = "riscv32")]
mod system;

#[cfg(target_arch = "riscv32")]
pub use hci::{
    BluetoothEntropy, BluetoothHci, BluetoothHciRestartError, BluetoothHciService,
    BluetoothHostTransport, CONTROLLER_TO_HOST, HOST_TO_CONTROLLER, OUTPUT, PACKET,
    start_bluetooth_hci,
};
#[cfg(target_arch = "riscv32")]
pub use oer_bluetooth_runtime::{Exhaustion, ServeExit, Served};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_bluetooth_radio::RadioFault;
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_bluetooth_runtime::{BluetoothFault, BluetoothRuntimeFault};
#[cfg(target_arch = "riscv32")]
pub use system::{
    BluetoothFailStop, BluetoothInterruptFault, BluetoothMemoryError, BluetoothParked,
    BluetoothRunnerFault, BluetoothStartError, BluetoothStartFailure, BluetoothStopError,
    BluetoothStopFailure, BluetoothSystem, BluetoothSystemBackend, BluetoothSystemDriver,
    BluetoothSystemFault, BluetoothSystemMemory, BluetoothSystemRetained, BluetoothSystemRuntime,
    BluetoothSystemUninstalled, EVENTS, ITEMS, MODEM_TIMER_CAPACITY, start,
};
