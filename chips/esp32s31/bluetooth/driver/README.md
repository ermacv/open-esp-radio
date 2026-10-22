# ESP32-S31 Bluetooth hardware engine

This crate owns chip hardware sequencing and affine radio publication states
below the Link Layer: Controller HAL and time, the BLE PHY, interrupts,
the modem low-power timer and the hardware scheduler with its event executor,
list-zero hardware execution and finished lists. Role value builders select
the physical profiles through typed HAL operations. It also
keeps the vendor Controller's event policy as pure values: coexistence
priorities, the coexistence schedule status bits and the connection-event
allowances. No radio role, runtime or composition in this repository lowers
the portable LE radio values onto the executor, and no Controller core serves
HCI over them.

Portable HCI codecs and LE Link Layer codecs live in
[`crates/protocols/bluetooth`](../../../../protocols/bluetooth/).

The module table below is the engine API map. `FEATURES.md` records current
limitations.

| Module under `src/` | Responsibility |
| --- | --- |
| `controller_hal` | Controller HAL component initialization on the HAL's clocked Bluetooth client of the shared radio |
| `controller_time` | Event-driven Controller-time latch and scheduler-epoch projection |
| `ble_phy` | BLE PHY engine initialization, activation into the runtime owners and the retained PHY membership and BLE PHY/DF graph |
| `phy` | Entry into and exit from the shared PHY domain and BTBB |
| `interrupt` | Chip interrupt state and hardware handling |
| `modem_timer` | Controller modem low-power timer task over the published timer owner |
| `scheduler` | The scheduler core over controller time and the finished-list worker; the physical list protocol and publications belong to the HAL Bluetooth engine |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Task and inactive interrupt owners split from the clocked client |
| `runtime_resources` | Interrupt-shared event cells reused by every epoch, and the task and timer workers of one powered epoch; the HAL engine owns list-zero publications and the global receive chains |
| `shutdown` | Retirement of a stopped epoch (Controller output, PHY and BTBB exit) and the Controller reset that returns the clocked client and the BLE PHY allocations |
| `roles` | Detached value builders for DTM, advertising, scanning and connection events, and role-to-list selection |
| `coexistence` | The vendor's per-lane PTI coexistence priorities of advertising, passive scan and connection events (`coexAdv`, `coexScan`, `coexConn`) |
| `coex_status` | The vendor's Bluetooth LE coexistence schedule status bits (`coexHook`) and the change between two activities |
| `connection_policy` | Connection-event allowances and span (`ble_28.o`, `ble_3.o`) and the vendor Direct Test Mode notes |

`TaskResources::bind_engine` joins the unique controller-register borrow to
one `Placed<BluetoothArena>` after HAL initialization. This crate enables
the HAL's `bluetooth-engine` feature. That engine owns its physical graph,
list transactions, address publication and completion return. Hardware heads
resolve to logical identities inside HAL. A role builder receives a
detached value seed; it never borrows a controller layout. Affine pool and
epoch scopes prevent foreign and stale leases, and release consumes a lease.

The existing `initialize_ble_phy_engine` / `activate` / `shut_down` lifecycle
retains separate PHY and DF storage owners. It does not bind the aggregate
engine; `bind_engine` borrows the task resources for the engine's lifetime.
Standalone HAL component placement and publication APIs remain public.

The scheduler still lends the stable interrupt owner inside HAL, under the
Sources exception. Its execution lock is the hardware protocol lock. The
scheduler and modem-timer wake cells retain their `IrqSignal` handoff.

## Controller time

Controller time uses the HAL period's software conversion selector. The
standalone profile has two raw ticks per microsecond; its hardware divider is
a separate setting. Scheduler epoch updates retain fractional raw ticks, and
earlier fractional timestamps round down before deadline projection.

Physical event spans are non-empty and at most the signed raw-counter
half-range. The duration projection accepts a `RadioDuration<LeRadio>`, checks the full
`u64` value and fractional carry, and narrows only after proving the result
fits. Absolute scheduler timestamps keep their reviewed modular projection;
that representation does not authorize wrapping a portable radio instant.
First-receive and recurring waits validate their complete geometry before
encoding the controller-SRAM fields.

## Bring-up and teardown order

A composition runs the typed primitives of this crate in the vendor order of
[`bt.c`](https://github.com/espressif/esp-idf/blob//components/bt/controller/esp32s31/bt.c),
[`btdm_lp.c`](https://github.com/espressif/esp-idf/blob//components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c),
[`modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob//components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c)
and
[`Kconfig.mac`](https://github.com/espressif/esp-idf/blob//components/esp_hw_support/port/esp32s31/Kconfig.mac).

Start, while retaining the shared radio-domain owner:

1. power up the Bluetooth partition, then enable its clocks;
2. initialize the Controller HAL, the scheduler over the epoch's runtime
   cells, then the modem low-power timer hardware;
3. prepare the shared PHY, join it (tracking the concurrent PHY when the
   join reports it due), then initialize and activate the BLE PHY engine with
   the public address;
4. enable coexistence for Bluetooth, as `esp_bt_controller_enable` does; a
   later start failure is fail-stop and keeps it enabled;
5. publish the interrupt owners and the timer into stable
   storage, then bind the CPU routes.

The interrupt sources are `MODEM_BT_MAC` (source 124), `MODEM_LP_TIMER`
(source 127) and `MODEM_BT_MAC_INT1` (source 133), bound as one same-core set
at level three on core 0 to the primary, modem-timer and default NRT
handlers. The dispatcher is complete before the first route is enabled, so
an interrupt pending at once observes it. Once published, the storage stays
reserved after retirement: old static Controller borrows outlive it until a
board reset.

While it runs, every change of the active roles moves the Bluetooth LE
coexistence status bits ([`coex_status`](src/coex_status.rs)), and
`coex_register_ble_cb`'s start and stop callbacks tell the Controller whether
another radio shares the antenna, which selects the coexistence priorities
([`coexistence`](src/coexistence.rs)).

Stop reverses it:

1. settle the modem timer, disable the three routes and clear the
   dispatcher, then retire the timer and the interrupt register owners;
2. clear every Bluetooth LE coexistence status bit, retire
   the epoch (Controller output, PHY and BTBB exit), then disable
   coexistence, as `coex_disable` after the stack is disabled; close the PHY
   when Bluetooth was its last client;
3. shut down the Controller (reset returning the clocked client and the BLE
   PHY allocations), disable the clocks and power the partition down.
