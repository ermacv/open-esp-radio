# ESP32-S31 Bluetooth hardware engine

This crate owns chip hardware sequencing and affine radio publication states
below the Link Layer: Controller HAL and time, the BLE PHY, interrupts,
the modem low-power timer and the hardware scheduler with its event executor,
list-zero hardware execution and finished lists. Role value builders select
the physical profiles through typed HAL operations. It also
keeps the vendor Controller's event policy as pure values: coexistence
priorities, the coexistence schedule status bits and the connection-event
allowances. `port::runner` lowers the portable DTM values onto this executor.
The generic port queue adapter and physical runner have separate ownership;
there is no connected hardware HCI transport.

Portable HCI codecs and LE Link Layer codecs live in
[`protocols/bluetooth`](../../../../protocols/bluetooth/).

The module table below is the engine API map. `FEATURES.md` records current
limitations.

| Module under `src/` | Responsibility |
| --- | --- |
| `controller_hal` | Controller HAL component initialization on the HAL's clocked Bluetooth client of the shared radio |
| `controller_time` | Event-driven Controller-time latch and scheduler-epoch projection |
| `ble_phy` | BLE PHY engine initialization from a settled Bluetooth membership and activation into runtime owners |
| `port` | One-credit affine LE command/terminal queues, lifecycle supervision and the P2 physical DTM runner |
| `radio` | Controller lifecycle parked in the shared radio backend; matching-installation runtime handover and retirement |
| `interrupt` | Chip interrupt state and hardware handling |
| `modem_timer` | Controller modem low-power timer task over snapshots and unarmed plans |
| `scheduler` | The scheduler core over controller time and the finished-list worker; the physical list protocol and publications belong to the HAL Bluetooth engine |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Task and inactive interrupt owners split from the clocked client |
| `runtime_resources` | Interrupt-shared event cells reused by every epoch, and the task and timer workers of one powered epoch; the HAL owns physical list publications and receive-chain graphs |
| `shutdown` | Retirement of a drained epoch and Controller output, followed by the actor-ordered Controller reset that returns the clocked client and the complete arena |
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

The arena profile is selected by the `S` capacity in
`BluetoothArena<A, C, N, S, R, O>`. The DTM-only profile uses all six
capacities set to zero. It retains the DTM graph, PHY, direction-finding
workspace and device table, but excludes scanner and global RX graphs and
their packetless cursors. Other role APIs require `S = 1`; the type system
prevents building them on the DTM-only profile. Both profiles use the same
generated layouts, phased writers, compressed pointers and placement rules.

`Engine::submit_dtm` is specific to the DTM-only profile. The caller stops
and resumes the scheduler before submitting its single head; a fresh P3
snapshot must prove idle. Busy refusal returns CPU custody without starting
a live lock/modify transaction. Empty role pools contribute no scheduler
dispatch target. Shared scheduling uses `Engine::submit` and its live-list
protocol.

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

One interrupt table entry binds `interrupt::BluetoothInterrupts` once, at
start: the controller bank's primary (124) and NRT (133) sources, the modem
low-power timer (127) and the shared radio doorbell (`FROM_CPU` line 3), all
on one line at level three on core 0. Its handler,
`BluetoothInterrupts::service`, lowers the doorbell, applies the task's unarmed
timer plan, steps the timer, then takes one gate request, serves one request of
the scheduler channel and samples and acknowledges the primary status and the
NRT snapshots. A same-entry dispatch therefore refuses stop with
`TimerDispatchPending` while retaining its work; a clear status writes nothing.
The part holds a gate for the bank and one for the timer, and touches a register owner only while
its gate holds it.

`interrupt::BluetoothInterruptGate` is the task's end. The part holds the
doorbell line's lowering half from the start; the task's end exists only
once the router's `Bound` proof of the part made the doorbell's ringing half
(`BluetoothInterruptEnds::attach`). Opening an epoch hands the handler the
unprepared bank, the HAL's channel endpoint and the initialized timer: the handler prepares the
controller output, then starts the runtime timer. Preparation already arms
the independent rollover lane with the counter stopped, after sampling and
clearing both inherited pending bytes with whole-word zeros. The removed
state is exposed by `stale_pending_observation` and the BLE PHY initialization
report. The timer register owner and authoritative epoch
stay in the handler while value-only snapshots and unarmed plans cross through
`interrupt::ModemLpTimerPort`. P2 only mutates the queue and publishes
expirations. The handler applies the sourced disable/sample/program/command
transaction. Repeated entries during planning acknowledge and retain new
pending/rollover facts; their software work is published after the in-flight
plan returns. The route is never removed and nothing is
masked at the interrupt controller: the task and the handler share no lock,
only one-slot queues, the doorbell and the static reply signal.

The LP stop transaction writes the sourced all-ones disable image at `+0x14`
and the vendor deinit command at `+0x04`, then fences and samples both
pending bytes into the returned type. Whole-word zeros at `+0x24` and `+0x2c`
and another fence complete the measured quiet state. Pending observations
pass from the stop proof through `ControllerRetired::pending_observation`,
`ControllerResetTimerPending::pending_observation` and
`EngineResetTimerPending::pending_observation` to the reunited result and
`ControllerLifecycle::into_reset`; Controller reset does not clear LP pending
or stop its counter with LP clocks on. No exhaustive all-source or concurrent-arrival
claim is made. SOURCE: `HW6_S31_LP_QUIET_20261026` and
`HW6_S31_LP_RESET_RETENTION_20261026`, run `hw6-20261026-reset-02`
(`oer-notes/research/hw-ble-lp-reset.md`). The stopped owner acknowledges
rollover-only pending without dispatch or rearming. Software pending (`+0x24`) or an unknown nonzero status uses the
gate's existing fault path; its first typed pending/status fault remains
available through `BluetoothInterrupts::stopped_timer_fault` and the
composition's `stopped_timer_fault` observation cell. `AwaitingPlan` can stop
only if neither its original work nor retained arrivals request dispatch; retiring rollover-only
work discards only the ending epoch's time projection. Retirement consumes
its late queued snapshot before another epoch can open.

`Engine::install_scheduler_interrupts` binds a static single-request channel
to its installation epoch. `SchedulerDoorbell` rings the doorbell and waits
on a dedicated P2 `IrqSignal`. IRQ-bank changes and fresh diagnostic samples
return durable, affine replies. The installation epoch and a command number
that never wraps or resets reject stale replies. A dropped pending exchange
retains its seat and prevents retirement. Scheduler hardware locks retain
their protocol meaning. Scheduler and modem-timer wake cells keep their
independent `IrqSignal` handoffs.

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

The `bluetooth` feature of `oer-esp32s31-radio-system` composes this
lifecycle into its single P2 actor. `BluetoothAttachment` holds Bluetooth power,
coexistence and `BluetoothPhyMembership`; `ControllerLifecycle` holds the
branded Engine and its initialization states. The membership cell carries
only configuration and installation identities.

Start:

1. validate the permitted LE band and common conducted-power policy; the
   first client registers/calibrates the PHY, later clients retain its RF epoch;
2. settle due shared tracking, enter Bluetooth common power without Wi-Fi
   reset, then enable the Bluetooth clocks;
3. bind the arena through the scoped factory, initialize Controller HAL,
   scheduler and modem low-power hardware, and enter Bluetooth coexistence;
4. join BTBB and the PHY client set, finish initial/due tracking, then consume
   `BluetoothRfReady` to initialize the BLE PHY and public address;
5. after the actor answers tracked readiness, take the runtime once, bind its
   request channel and open the interrupt handler's gates with the bank, the
   channel endpoint and the timer.

The interrupt table binds the handler's part once per boot, before any
epoch; its storage stays reserved for every later epoch.

While it runs, every change of the active roles moves the Bluetooth LE
coexistence status bits ([`coex_status`](src/coex_status.rs)), and
`coex_register_ble_cb`'s start and stop callbacks tell the Controller whether
another radio shares the antenna, which selects the coexistence priorities
([`coexistence`](src/coexistence.rs)).

Stop:

1. stop the physical scheduler through P3, drain events, RX/TX and finished
   lists, and retire the drained modem-timer task (its owner stays with the
   handler);
2. `shutdown::retire` checks the workers and the stopped scheduler, releases
   the Controller output in alternating steps (the handler's idle sample, the
   task's list heads, the handler's mask, the task's RUN disable, the
   handler's release), closes the bank's gate (the released bank and the
   channel endpoint leave the handler), reunites both quiet channel endpoints
   and has the handler stop the counter and both measured LP delivery lanes. A fault the handler
   acknowledged during the epoch refuses the release. Return the retired
   Controller to `ControllerLifecycle`; only the exact running installation
   is accepted;
3. the actor leaves BTBB/PHY and resets the Controller; the lifecycle then
   takes the timer from its handler, which gives it up only after the reset,
   and the actor releases Bluetooth clocks, coexistence and common power and
   closes RF only for the last member.

The reset returns the complete arena and the task's end of the interrupt
handler's gate, which the next installation takes again: the handler's part
stays bound for every epoch. `into_reset` and `BluetoothAttachment::into_stopped`
allow a fresh scoped installation after the attachment loan is normally detached;
the shared actor and Wi-Fi membership remain in their original storage. A started RF failure poisons the
shared domain; a local Controller reset cannot clear it. A late retirement
returns its complete old graph and never satisfies a newer installation's leave.

## DTM port adapter

`port::Backend` implements the shared port contract over a P2 `Device`.
P1 receives only command/event endpoints and an independent controller-clock
client. One credit reserves both command and terminal storage until terminal
dequeue. Cancelled FIFO waiters release their seat; rejected terminals and
indeterminate physical outcomes retain their actual affine owners. An
abandonment supplies no synchronous DMA/IRQ exclusion proof.

`port::runner::Runner` owns the branded powered Controller runtime and one
actual DTM lease. It uses the shared LL window planner, fresh generation-keyed
Controller-time latches, fenced P3 admission/completion and exact stopped-graph
return. RX grants no TX-power authority. Unknown hardware completion status
faults the runner. The independent clock-request hook is served during IRQ
waits as well as between operations. Pool and epoch brands stay in the P2
owner; they do not cross static queues.

`port::mailbox` binds the supervisor to the retained physical worker. P1 alone
owns clock bookkeeping, including activation before enable and retirement
after drain. Retirement waits for an abandoned clock answer before returning
the poster. P2 constructs an epoch-keyed endpoint without entering that cell
and serves correlated clock requests through the mailbox.

`firmware/apps/b0` composes actor join/tracking, the interrupt gate, H4
Reset/Rand and 1M DTM commands, End, leave/reset and arena return. Its
two-boot RF/count gate is qualified; hardware port conformance is unqualified.
