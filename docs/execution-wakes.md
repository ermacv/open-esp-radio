# Task and interrupt notifications

The execution levels are in [architecture](architecture.md#execution). Task
notifications that travel upwards use `oer_portable_sync::PriorityWaker`. It has one
serialized consumer on one hart. A higher-priority producer completes before
interrupted registration resumes; a lower-priority producer may remain suspended. Producers publish input before notifying; the consumer registers
before its Acquire check of that input. Notifications can coalesce and do
not store payloads. The two slots keep registration away from a suspended
producer's `AtomicWaker` operation; a nesting count retains each slot until
all its notifications finish. No lock, critical section or unsafe code is
needed. Cross-core memory safety does not imply this priority progress
guarantee.

`oer-portable-sync` is a separate portable crate because memory ownership, protocol
ports and chip drivers consume it independently. Its only dependency is
`oer-portable-waker`, whose `AtomicWaker` holds one task's waker and is the one
place of this notification's `unsafe`; neither pulls memory's buffer machinery or
Embassy into a protocol contract.

A call to `AtomicWaker` makes a fixed number of atomic operations and at most one
waker call, with no retry loop of its own. That bound is algorithmic, not a
machine-time bound: a strong compare-and-swap is a reservation loop on RISC-V.
A registration owes its task a wake when a wake came during it, and carries that
wake through the waker it was given. Its model checks run with
`cargo test -p oer-portable-waker --features loom --release`.

## Notification paths

Paths in the table are relative to the repository root. P1 includes the
station service, network stack and applications; the word “radio” in a type
name does not establish that its task runs on P2.

| Place | Producer → waiter | Risk, mechanism and progress |
| --- | --- | --- |
| `chips/esp32s31/radio/ieee80211/backend/src/port/lent.rs`: network return | Lent receive owner dropped by the network P1 → datapath actor P1 polling the port's facts, `poll_lend_room` or `poll_lent_returned` | Ordinary AtomicWaker for the same-executor P1 waiter; the drop lowers the atomic count before it wakes. A network on another hart raises the actor's executor signal instead. The owner is dropped in a task, never in an interrupt handler. |
| `chips/esp32s31/radio/ieee80211/backend/src/rx/control.rs`: runner | Stop handle → receive runner | Upward wake where a composition uses it, protected by `PriorityWaker`. No image does: `WifiRxArena::bind`'s stop handle and frame ends are discarded in `chips/esp32s31/radio/ieee80211/station/src/radio.rs`. |
| Same file: reporter | Receive runner → application | Ordinary AtomicWaker, descending priority. No image uses it, as above. |
| `chips/esp32s31/radio/ieee80211/backend/src/rx/upper.rs`: frame queue | Receive runner → frame consumer | Ordinary AtomicWaker, descending priority. No image uses it, as above. |
| `portable/memory/src/request_slot.rs`: Doorbell | Clients P1 → radio-system actor P2 | The one upward wake in an image, protected by `PriorityWaker`; clients OR durable request bits before waking the actor. BLE clients are not linked yet. |
| Same file: answer | Radio-system actor P2 → requesting client P1/P2 | `PriorityWaker` also supports a higher-priority client without changing the request-slot contract. |
| `protocols/network/runtime/src/socket.rs`: result places, credits | Network service or an observer dropped or read → one observer, or the socket waiting for a credit | Same hart: a socket and its service share one, and an observer is affine to its one place. One cell per place and one for credits, so registrations of a cell never differ. The word is published before the wake and checked after registering |
| `protocols/radio/port/src/slot.rs`: p2 | Frontend/body P1, registration teardown P2 → registered owner P2 | Upward wake, protected by `PriorityWaker`; commands, returns, retirement and quarantine publish their state before notification. No image holds a `PortSlot` yet. |
| Same file: join | Registered owner P2 → frontend P1 | Ordinary AtomicWaker, descending priority. |
| `portable/embassy/net/src/owned.rs`: radio_waker, network_waker | HIL station/network tasks P1 ↔ P1 | `oer_portable_sync::PriorityWaker` over atomic link state. The image retains the same P1 composition; endpoint and link-controller loans are affine and local. |
| RX in `portable/embassy/net/src/owned.rs` | Station producer and network consumer P1 ↔ P1 | Affine `WakingSpscQueue` ends and ordinary AtomicWakers. TX has no queue here: a frame goes to the device's transmit half in the call that sent it. |
| Xarxa's pinned `PacketPoolWaiter`, through `owned.rs` and the stack's general pool | Packet destruction P1 → stack runner P1 | Dependency's ordinary AtomicWaker, same executor; its `register` may wake its own task. `oer-portable-waker` does not cover it. The RX pool's waiter is claimed and never registered. |
| `chips/esp32s31/app/src/lib.rs`: `APPLICATION` | IP status writer → the application's `status`, `wait_ipv4`, `wait_link_down` | One cell through `&self`, so its live registrations must wake alike. `embassy-futures`' `select` and `join` hand their children the task's waker, and replace no one. A combinator with a waker per child, such as `FuturesUnordered`, loses a wake here: the last registration replaces the other, and a ready wait stays pending. Across harts it is a `PeerNotify`. |
| `chips/esp32s31/radio/ieee80211/station/src/lib.rs`: `StartState`, `StopState`, `NetworkStopState` | Station service and network P1 ↔ application or actor P1 | `PriorityWaker`s after atomic facts, or `AcrossHarts` notifies on the network's stop. |
| `protocols/ieee80211/runtime/src/station/control.rs`: `policy_waker`, `control/operation.rs`: `Operations` | Application ↔ station service, P1 | `PriorityWaker`s after atomic facts. |
| `portable/embassy/net/src/ip.rs`: `State.runner` | The service's `Control` → its runner, P1 | `PriorityWaker` after the atomic request. |
| `chips/family/platform/executor/src/peer/obligation.rs` | Peer client task → liveness supervisor task, both of one executor `C` | No waker cell: the supervisor's task handle (`C::current`) sits in a `ContextCell` of `C`'s context and every seat write wakes it with `C::wake`. Writes take the checked proof of `C`'s context (`signal::enter`) and the supervisor registers only after `signal::enter_task`, so both run in one executor context, never at once, and the wake stays on its hart. |
| `portable/memory/src/waking_spsc.rs`, `slot_queue.rs`, `lib.rs` | Network/application/service P1 ↔ P1 | Ordinary AtomicWakers are the default consumer and producer notifies (`lib.rs` re-exports the type); same-hart endpoints currently run on P1. Ends on two harts name executor-signal notifies instead and use no waker. The generic queue must not be composed across priorities with an upward wake. |
| `protocols/ieee80211/runtime/src/station/notifies.rs` | Station P1 ↔ network P1 on one hart | `SameHart` names ordinary AtomicWakers for the network channels' queue ends and `PriorityWaker`s for the link and stop. `AcrossHarts` names executor-signal notifies and no waker. |
| `chips/family/app/src/net.rs`: IP waiters | IP service task → `wait_ipv4`/`wait_link_down` waiters, all on the application executor (P1) | Up to `K` waiter `Waker`s, each under its wait's number, in a `ContextCell` checked with `Entered<X>`; a wait refreshes its own slot and frees it when it ends or is dropped. The service's status callback takes them all and wakes them after the borrow ends. A full table refuses the wait with a typed error. |
| `portable/time/src/connected_tasks.rs` | Parent/worker tasks on the composition's executor | `PriorityWaker`s `stop_waker` and `stopped_waker`, after the state they tell. No hardware P1 → P2 composition uses these cells. |
| `protocols/ieee80211/runtime/src/monitor/{capture,injection}.rs` | Monitor capture/injection producer → consumer on the composition's executor | `oer_portable_memory::WakingSpscQueue` (ordinary AtomicWakers) and a `ClaimCell` binding; no lock. A composition must not compose them across priorities with an upward wake. |
| `protocols/hil/agent/src/console.rs`; HIL request/console Channels | Synchronous callers only queue; the sole async console and dispatch run on P1 | Embassy Mutex/Channel and the console's places use CriticalSectionRawMutex, which masks the hart, including P3, during a P1 operation. Wakers run after that lock. Logger waits descend to P1; this remains a checked deviation from the blocking critical-section restriction. |
| `chips/esp32s31/radio/ieee80211/backend/src/rx/returns.rs`, Wi-Fi MAC/power interrupt consumers | Held-frame return P1 → P2; MAC/power P3 → P2 | `IrqSignal::raise` ORs facts and pends P2 without a waker call. Registration of the polling task's handle and its wake occur only in the bound P2 executor. |
| `chips/esp32s31/radio/bluetooth/backend/src/interrupt/{halves,wake}.rs`, `modem_lp_timer_queue.rs` | Scheduler request P2 → doorbell P3; scheduler/timer reply P3 → P2 | P2 rings the hardware doorbell; P3 owns scheduler registers and raises typed IrqSignals back. No task waker is invoked on P3. |
| `chips/family/radio/ieee802154/engine` and `protocols/ieee802154/{port,mac}` | Engine events in P3; MAC state advanced by its caller | Synchronous callbacks/polling, no atomic-waker task cell. The model's RefCell wakers belong to its single-threaded host world. |
| `chips/family/platform/executor/src/time_driver.rs`, `timer_queue.rs` | Timer alarm P3/P1 → owning executor P2/P1 | Alarm latches/pends; the executor services its own timer queue. Expired timers and foreign-context rejection call the task waker directly without an AtomicWaker or a lock held by a lower-priority producer. |
| `chips/family/platform/interrupt-table/src/signal.rs` | Higher handler (`FromHandler`), lower task (`FromTask`), or same-level `Exempt(ExecutorLevel)` handler (`FromExecutorLevel`) → bound executor on the same hart | Shared state is an atomic fact word. A same-level handler cannot preempt the executor's round; it publishes facts and pends the next round. Only the receiving executor touches its task handle (`SignalExecutor::Task`, woken by a direct call, no `Waker`); the raising context never owns it, and a waker that names no task of the executor is refused. |
| `protocols/ieee80211/testing/model/src/lib.rs`: lent receive buffers | A host test's lent-buffer drop → the model port's `poll_lent_returned` waiter | Ordinary AtomicWaker in the single host model world; the count is atomic only because a lent buffer is `Send`. |
| `protocols/radio/port/src/pool/fifo.rs`, `portable/embassy/net/src/datapath/memory.rs`, BLE/Wi-Fi models, `portable/time-virtual` | Parts of one task or one host model world | RefCell/Option or MultiWakerRegistration, not Sync. No shared cell crosses execution levels. |
| Cooperative `cx.waker().wake_by_ref()` in driver/HIL/probe steps | Task → itself | Direct executor waker, no shared registration held during the callback. |

The network device, radio runner, RX publisher and link controller cannot be
copied or sent to another thread. The RX queue's ordinary wakers and the pool
waiter remain in one P1 composition; no new cross-priority distribution is
admitted.

A cell holds one waker, so all its live registrations must be interchangeable
for waking; otherwise each needs a registration of its own. One task does not
give this: a combinator may hand each child its own waker. A queue's affine ends
prove it by type. The cells reached through `&self` keep it by composition.

Direct `embassy-sync` dependencies remain in `portable/cooperate`,
`protocols/hil/agent` and the three HIL images. The
layer test checks those exact edges against its shrinking deviation list;
the absence of priority preemption does not admit a new mutex-backed path.

## Static check

`cargo test -p oer-tools-test-layers` inventories raw AtomicWaker spellings in
production source under `chips/`, `portable/`, `protocols/`, `oer/`, `examples/`
and `hil/`. Only reviewed files with their recorded occurrence counts are
admitted; new files or additional uses require review. It sees neither a
`PriorityWaker` nor a cell a queue's default type parameter names. The upward
fields are separately checked for `PriorityWaker`. This is a grep gate, not
priority inference:
a change of executor placement, a type alias or a renamed dependency needs the
composition review described above. It does not infer priorities from a
Rust type name or audit the source of external dependencies.
