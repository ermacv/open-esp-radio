# ESP32-S31 Bluetooth hardware engine

This crate owns chip hardware sequencing and affine radio publication states
below the Link Layer: Controller HAL and time, the BLE PHY, interrupts,
the modem low-power timer and the hardware scheduler with its event executor,
list-zero hardware execution and finished lists. It knows no LE role. It also
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
| `scheduler` | Event executor (idle and live insertion, cancellation, list deletion, completion, stopped state), ordered hardware-list mirror, raw windows, timing policy, list-zero hardware execution and finished lists |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Task and inactive interrupt owners split from the clocked client |
| `runtime_resources` | Interrupt-shared event cells reused by every epoch, and the workers and list-zero publications of one powered epoch; the powered task endpoint performs executor steps, starts the idle scheduler, stops it, publishes the global receive chains and retires its task owner |
| `shutdown` | Retirement of a stopped epoch (Controller output, PHY and BTBB exit) and the Controller reset that returns the clocked client and the BLE PHY allocations |
| `memory` | Controller-SRAM storage owners and completion values from the memory crate |
| `coexistence` | The vendor's per-lane PTI coexistence priorities of advertising, passive scan and connection events (`coexAdv`, `coexScan`, `coexConn`) |
| `coex_status` | The vendor's Bluetooth LE coexistence schedule status bits (`coexHook`) and the change between two activities |
| `connection_policy` | Connection-event allowances and span (`ble_28.o`, `ble_3.o`) and the vendor Direct Test Mode notes |

A caller reaches hardware only through these typed primitives. The powered
task endpoint publishes an address only after it resolves to a submitted item
of the caller's scheduler item space, whose pools bind `'static` storage. It
retains every list-zero publication from the action that creates it to the
action that ends it, so an action out of transaction order changes no
register. Each wait observation joins a fresh task-side read with a fresh
scheduler-state read from the stable interrupt owner; the scheduler wake only
tells the worker when to observe again. Operations
that discharge an `unsafe` obligation, such as publishing an interrupt owner
or preparing Controller output, are safe functions of this crate. The
exception is `ControllerPoweredTaskRuntime::publish_scan_start`, an `unsafe
fn`: its caller must have reset the scanner graph and published the receive
chains, which the removed radio role did.

The separate [`memory`](memory/) crate retains controller-SRAM codecs.

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

Start, under the shared radio's lock:

1. power up the Bluetooth partition, then enable its clocks;
2. initialize the Controller HAL, the scheduler over the epoch's runtime
   cells, then the modem low-power timer hardware;
3. prepare the shared PHY, join it (tracking the concurrent PHY when the
   join reports it due), then initialize and activate the BLE PHY engine with
   the public address;
4. enable coexistence for Bluetooth, as `esp_bt_controller_enable` does; a
   later start failure is fail-stop and keeps it enabled;
5. outside the lock, publish the interrupt owners and the timer into stable
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
2. under the lock, clear every Bluetooth LE coexistence status bit, retire
   the epoch (Controller output, PHY and BTBB exit), then disable
   coexistence, as `coex_disable` after the stack is disabled; close the PHY
   when Bluetooth was its last client;
3. shut down the Controller (reset returning the clocked client and the BLE
   PHY allocations), disable the clocks and power the partition down.
