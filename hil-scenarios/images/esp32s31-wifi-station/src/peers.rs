//! The signals the two harts raise for each other's tasks, and the queue
//! notifies over them (`PeerNotify`).
//!
//! No waker crosses harts: a queue end whose peer runs on the other hart
//! waits on its own task's executor signal, which the peer raises after
//! publishing. One signal wakes one task, so every end that notifies
//! through a signal is polled by that signal's one task:
//!
//! | Signal | Receiving task | Raised by |
//! | --- | --- | --- |
//! | [`NETWORK`] | hart 1's `network` | the boot hart's application tasks: the station work, the console, the relay |
//! | [`STATION`] | the boot hart's `station_work` | hart 1's `network` |
//! | [`RELAY`] | the boot hart's `relay` | hart 1's `network` |
//! | [`DRIVER`] | the boot hart's radio `driver` | hart 1's `network`, and the station work below it |
//!
//! A lent receive buffer the station itself drops (a delivery its network
//! queue refused) raises [`STATION`] from the station work's own task:
//! that executor's next round wakes the task, a raise the signal's
//! declaration cannot name.

use oer_esp32s31_compose::Esp32s31;
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_espressif_interrupt_table::signal::{FromPeerHart, FromTask, IrqSignal, PeerNotify};

/// The boot hart's application executor (thread mode, `FROM_CPU0`).
pub(crate) type AppExecutor = Executor<Esp32s31, 0>;
/// The boot hart's radio executor (P2, `FROM_CPU2`).
pub(crate) type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
/// Hart 1's network executor (P2 of hart 1, `FROM_CPU1`).
pub(crate) type NetworkExecutor = InterruptExecutor<Esp32s31, 1>;

/// The notify of hart 1's network task, as each of its queue ends names it.
pub(crate) type NetworkNotify = &'static PeerNotify<NetworkExecutor>;
/// The notify of the boot hart's station work.
pub(crate) type StationNotify = &'static PeerNotify<AppExecutor>;
/// The notify of the boot hart's relay task.
pub(crate) type RelayNotify = &'static PeerNotify<AppExecutor>;
/// The notify of the boot hart's radio driver.
pub(crate) type DriverNotify = &'static PeerNotify<RadioExecutor>;

oer_espressif_staged_runtime::placed_static! {
    static NETWORK_SIGNAL: IrqSignal<NetworkExecutor> = IrqSignal::new::<FromPeerHart<AppExecutor, crate::ExecutorWake>, crate::NetworkToken>(), in critical_data "hart_peers";
    static STATION_SIGNAL: IrqSignal<AppExecutor> = IrqSignal::new::<FromPeerHart<NetworkExecutor, crate::NetworkToken>, crate::ExecutorWake>(), in critical_data "hart_peers";
    static RELAY_SIGNAL: IrqSignal<AppExecutor> = IrqSignal::new::<FromPeerHart<NetworkExecutor, crate::NetworkToken>, crate::ExecutorWake>(), in critical_data "hart_peers";
    static DRIVER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromPeerHart<NetworkExecutor, crate::NetworkToken>, FromTask<AppExecutor, crate::ExecutorWake>), crate::RadioToken>(), in critical_data "hart_peers";
    /// Hart 1's network task: the station's network end, the IP stack and
    /// the load server.
    pub(crate) static NETWORK: PeerNotify<NetworkExecutor> = PeerNotify::new(&NETWORK_SIGNAL), in critical_data "hart_peers";
    /// The boot hart's station work.
    pub(crate) static STATION: PeerNotify<AppExecutor> = PeerNotify::new(&STATION_SIGNAL), in critical_data "hart_peers";
    /// The boot hart's relay of the load server's answers and requests.
    pub(crate) static RELAY: PeerNotify<AppExecutor> = PeerNotify::new(&RELAY_SIGNAL), in critical_data "hart_peers";
    /// The boot hart's Wi-Fi driver, when it asked for its lent buffers.
    pub(crate) static DRIVER: PeerNotify<RadioExecutor> = PeerNotify::new(&DRIVER_SIGNAL), in critical_data "hart_peers";
}
