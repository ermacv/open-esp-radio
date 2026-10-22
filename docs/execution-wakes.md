# Task and interrupt notifications

The execution levels are in [decisions](decisions.md#execution). Task
notifications that travel upwards use `oer_sync::PriorityWaker`. It has one
serialized consumer on one hart. A higher-priority producer completes before
interrupted registration resumes; a lower-priority producer may remain suspended. Producers publish input before notifying; the consumer registers
before its Acquire check of that input. Notifications can coalesce and do
not store payloads. The two slots keep registration away from a suspended
producer's `AtomicWaker` operation; a nesting count retains each slot until
all its notifications finish. No lock, critical section or unsafe code is
needed. Cross-core memory safety does not imply this priority progress
guarantee.

`oer-sync` is a separate portable crate because memory ownership, protocol
ports and chip drivers consume it independently. Its only dependency is the
atomic-waker implementation; it does not pull memory's buffer machinery or
Embassy into a protocol contract.

## Notification paths

Paths in the table are relative to the repository root. P1 includes the
station service, network stack and applications; the word “radio” in a type
name does not establish that its task runs on P2.

| Place | Producer → waiter | Risk, mechanism and progress |
| --- | --- | --- |
| `chips/esp32s31/ieee80211/driver/src/port.rs`: driver notification, RX/TX returns | Station P1 and driver P2 → driver P2 | Upward wake, protected by `PriorityWaker`. P2 can preempt a P1 notification; the notification keeps its original slot until it returns. |
| Same file: body notification | Driver P2 → station P1 | Ordinary `atomic_waker::AtomicWaker`. P1 cannot preempt P2; P2 may interrupt registration, which completes on resumption. |
| `chips/esp32s31/ieee80211/driver/src/rx/control.rs`: runner | Stop handle P1 → receive runner P2 | Upward wake, protected by `PriorityWaker`; repeated stop requests publish the same atomic flag. |
| Same file: reporter | Receive runner P2 → application P1 | Ordinary AtomicWaker, descending priority. |
| `chips/esp32s31/ieee80211/driver/src/rx/upper.rs`: frame queue | Receive runner P2 → frame consumer P1 | Ordinary AtomicWaker, descending priority. Returning a held frame takes the separate typed-signal path below. |
| `portable/memory/src/request_slot.rs`: Doorbell | Wi-Fi/BLE clients P1 or P2 → radio-system actor P2; W3 ready exchange | Upward wake, protected by `PriorityWaker`; clients OR durable request bits before waking the actor. |
| Same file: answer | Radio-system actor P2 → requesting client P1/P2; W3 ready reply P2 → P1 | `PriorityWaker` also supports a higher-priority client without changing the request-slot contract. |
| `protocols/radio/port/src/slot.rs`: p2 | Frontend/body P1, registration teardown P2 → registered owner P2 | Upward wake, protected by `PriorityWaker`; commands, returns, retirement and quarantine publish their state before notification. |
| Same file: join | Registered owner P2 → frontend P1 | Ordinary AtomicWaker, descending priority. |
| `firmware/apps/w1/src/measure.rs`: PHASE_WAKER | Phase writer P1 → timer observer P2 | Upward wake, protected by `PriorityWaker`; the phase is an atomic level checked after registration. |
| `portable/embassy/net/src/owned.rs`: radio_waker, network_waker | W3 station/network tasks P1 ↔ P1 | Embassy `GenericAtomicWaker<NoopRawMutex>`. No preemption between these users. This implementation locks a `Cell<Option<Waker>>`, invokes the callback while locked and restores its registration; it has no WAKING state. |
| Same file: tx_published and RX Channel; `owned/tx_queue.rs`, `owned/tx_budget.rs` | W3 network producer, station consumer and packet return P1 ↔ P1 | Embassy Signal/Channel and WakerRegistration under `NoopRawMutex`, same executor. Signal publishes Signaled before its callback; WakerRegistration takes its registration before waking. These mutex-backed paths are not admitted for upward notifications. |
| Xarxa's pinned `PacketPoolWaiter`, through `owned.rs` | W3 packet destruction P1 → RX publisher P1 | Dependency's ordinary AtomicWaker, same executor. Moving the publisher to P2 would require reviewing this path too. |
| `protocols/ieee80211/runtime/src/station_service/msdu_queue.rs` | W3 network/application/service P1 ↔ P1 | Ordinary AtomicWakers for consumer and producer; both endpoints currently run on P1. The generic queue must not be composed across priorities with an upward wake. |
| `protocols/ieee80211/runtime/src/connected_tasks.rs`, `monitor/{capture,injection}.rs` | Parent/worker and monitor tasks on the composition's executor | Embassy Signal/Channel under the supplied RawMutex. No hardware P1 → P2 composition uses these cells. A composition must not introduce an upward wake through them. |
| `portable/hil-agent/src/console/writer.rs`, `console.rs`; HIL request/console Channels | Immediate output may run at a higher level; the sole async logger/console and dispatch run on P1 | Embassy AtomicWaker uses CriticalSectionRawMutex, not the atomic-waker WAKING protocol. Logger waits descend to P1; the blocking critical-section restriction is separate from priority livelock. |
| `chips/esp32s31/ieee80211/driver/src/rx/returns.rs`, Wi-Fi MAC/power interrupt consumers | Held-frame return P1 → P2; MAC/power P3 → P2 | `IrqSignal::raise` ORs facts and pends P2 without a waker call. Registration, waker replacement/destruction and delivery occur only in the bound P2 executor. |
| `chips/esp32s31/bluetooth/driver/src/interrupt/{halves,wake}.rs`, `modem_lp_timer_queue.rs` | Scheduler request P2 → doorbell P3; scheduler/timer reply P3 → P2 | P2 rings the hardware doorbell; P3 owns scheduler registers and raises typed IrqSignals back. No task waker is invoked on P3. |
| `chips/espressif/ieee802154-engine` and `protocols/ieee802154/mac` | Engine events in P3; MAC state advanced by its caller | Synchronous callbacks/polling, no atomic-waker task cell. The model's RefCell wakers belong to its single-threaded host world. |
| `chips/espressif/executor/src/time_driver.rs`, `timer_queue.rs` | Timer alarm P3/P1 → owning executor P2/P1 | Alarm latches/pends; the executor services its own timer queue. Expired timers and foreign-context rejection call the task waker directly without an AtomicWaker or a lock held by a lower-priority producer. |
| `portable/interrupt-table/src/signal.rs` | Any raising context → bound executor | Shared state is an atomic fact word. Only the receiving executor touches its Option<Waker>; the raising context never owns it. |
| `protocols/radio/permit-pool`, `protocols/ieee80211/datapath/src/memory.rs`, BLE/Wi-Fi models, `portable/time-virtual` | Parts of one task or one host model world | RefCell/Option or MultiWakerRegistration, not Sync. No shared cell crosses execution levels. |
| Cooperative `cx.waker().wake_by_ref()` in driver/HIL/probe steps | Task → itself | Direct executor waker, no shared registration held during the callback. |

The generic Embassy network adapter also permits other RawMutex types. Its
existing mutex/owner limitations are documented in its
[README](../portable/embassy/net/README.md); those types alone do not prove a
valid cross-priority or cross-core composition. W3 selects NoopRawMutex and
keeps every adapter endpoint on P1. Neither an IRQ nor a P2 task may share
that NoopRawMutex state.

## Static check

`cargo test -p oer-test-layers` inventories raw AtomicWaker spellings in
production source under `chips/`, `portable/`, `protocols/` and `firmware/`.
Only reviewed files with their recorded occurrence counts are admitted; new
files or additional uses require review. The upward fields are separately
checked for `PriorityWaker`. This is a grep gate, not priority inference:
a change of executor placement, a type alias or a renamed dependency needs the
composition review described above. It does not infer priorities from a
Rust type name or audit the source of external dependencies.
