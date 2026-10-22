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
| `ble_phy` | BLE PHY engine initialization, activation into the runtime owners and the retained shared PHY membership |
| `phy` | Entry into and exit from the shared PHY domain and BTBB |
| `interrupt` | Chip interrupt state and hardware handling |
| `modem_timer` | Controller modem low-power timer task over the published timer owner |
| `scheduler` | The scheduler core over controller time and the finished-list worker; the physical list protocol and publications belong to the HAL Bluetooth engine |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Task and inactive interrupt owners split from the clocked client |
| `runtime_resources` | Interrupt-shared event cells reused by every epoch, and the task and timer workers of one powered epoch; the HAL owns physical list publications and receive-chain graphs |
| `shutdown` | Retirement of a stopped epoch (Controller output, PHY and BTBB exit) and the Controller reset that returns the clocked client and the complete arena |
| `roles` | Detached value builders for DTM, advertising, scanning and connection events, and role-to-list selection |
| `coexistence` | The vendor's per-lane PTI coexistence priorities of advertising, passive scan and connection events (`coexAdv`, `coexScan`, `coexConn`) |
| `coex_status` | The vendor's Bluetooth LE coexistence schedule status bits (`coexHook`) and the change between two activities |
| `connection_policy` | Connection-event allowances and span (`ble_28.o`, `ble_3.o`) and the vendor Direct Test Mode notes |

The driver enables the HAL's `bluetooth-engine` feature. Its
`*_event_image` modules select DTM role/PHY/phase, advertising channel order
and windows, and scanner filter, active/passive and start policy. They pass
detached values through closed HAL operations. A builder never borrows a
controller layout; HAL checks binding anchors before committing a value.
Affine pool and epoch scopes prevent foreign and stale leases, and release
consumes its lease. Hardware heads resolve to logical identities inside HAL.

`Engine::new` consumes the clocked client and one `Placed<BluetoothArena>`
with composition-selected capacities and fresh pool/epoch scopes. The
resulting engine and inactive IRQ bank enter `ControllerHalInitialized`;
HAL initialization rejects repeated use without discarding either owner.
The engine stays inside `TaskResources<M>` through PHY initialization,
activation, controller-time and finished-list workers, stop and shutdown.
PHY and DF storage cannot be extracted or initialized independently.

All graph publications and CPU completion return use that engine. A
finished-list receipt belongs to its capturing engine installation epoch. Retirement
requires a stopped and empty physical scheduler and idle workers. Successful
reset returns the same entire arena, zeroed in place, together with the
clocked client; constructor refusal returns its untouched inputs. Terminal
failures retain every hardware partition and the arena in absorbing
quarantine. There is no reset-failure extraction or late-stop recovery.

One `ControllerInterruptHalf` holds the IRQ register owner and the HAL's
request endpoint in stable P3 storage. Primary, NRT and shared radio doorbell
entries service that same half. The owning task engine at P2 retains the arena
and every scheduler transaction; it has no access to the IRQ register owner.
`Engine::install_scheduler_interrupts` binds a static single-request channel
to its installation epoch. `SchedulerDoorbell` rings the image's shared
`FROM_CPU` line 3 and waits on a dedicated P2 `IrqSignal`. The shared handler
acknowledges the software interrupt before visiting the radio halves.

IRQ-bank changes and fresh diagnostic samples return durable, affine replies.
The installation epoch and a command number that never wraps or resets reject
stale replies. A dropped pending exchange retains its seat and prevents
retirement. Stop and route removal return the P3 endpoint; both quiet halves
reunite before reset and channel reuse. Scheduler hardware locks retain their
protocol meaning. Scheduler and modem-timer wake cells keep their independent
`IrqSignal` handoffs.

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
5. bind the engine's request channel, publish its P3 half and the timer into
   stable storage, then bind the CPU routes.

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

1. stop the physical scheduler through P3 while the shared doorbell remains
   live, drain its finished lists and settle the modem timer; then disable
   the three BLE routes and remove BLE dispatch, retire the timer and P3
   half, and reunite both quiet request-channel endpoints;
2. clear every Bluetooth LE coexistence status bit, retire
   the epoch (Controller output, PHY and BTBB exit), then disable
   coexistence, as `coex_disable` after the stack is disabled; close the PHY
   when Bluetooth was its last client;
3. shut down the Controller (reset returning the clocked client and the
   complete arena), disable the clocks and power the partition down.
