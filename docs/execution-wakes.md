# Task and interrupt notifications

The execution levels are in [architecture](architecture.md#execution). Task
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
| `portable/memory/src/request_slot.rs`: Doorbell | Wi-Fi/BLE clients P1 or P2 → radio-system actor P2 | Upward wake, protected by `PriorityWaker`; clients OR durable request bits before waking the actor. |
| Same file: answer | Radio-system actor P2 → requesting client P1/P2 | `PriorityWaker` also supports a higher-priority client without changing the request-slot contract. |
| `protocols/radio/port/src/slot.rs`: p2 | Frontend/body P1, registration teardown P2 → registered owner P2 | Upward wake, protected by `PriorityWaker`; commands, returns, retirement and quarantine publish their state before notification. |
| Same file: join | Registered owner P2 → frontend P1 | Ordinary AtomicWaker, descending priority. |
| `portable/embassy/net/src/owned.rs`: radio_waker, network_waker | HIL station/network tasks P1 ↔ P1 | `oer_sync::PriorityWaker` over atomic link state. The image retains the same P1 composition; endpoint and link-controller loans are affine and local. |
| `owned/tx_queue.rs`, `owned/tx_budget.rs` and RX in `owned.rs` | HIL network producer, station consumer and packet return P1 ↔ P1 | TX metadata and its coalesced publication hint use `ContextCell` with a private checked `Entered<X>` proof. All Waker callbacks run after the borrow ends. Admission is atomic and uses `PriorityWaker`; packet destruction precedes credit return. RX uses affine `WakingSpscQueue` ends and ordinary AtomicWakers. |
| Xarxa's pinned `PacketPoolWaiter`, through `owned.rs` | Network packet destruction P1 → RX publisher P1 | Dependency's ordinary AtomicWaker, same executor. Moving the publisher to P2 would require reviewing this path too. |
| `chips/espressif/executor/src/peer/obligation.rs` | Peer client task → liveness supervisor task, both of one executor `C` | Ordinary AtomicWaker; the types bind both to the same executor, so they never run at once and the wake stays on its hart. The client wakes after every seat write (publish, then wake); the supervisor registers before each scan. |
| `portable/memory/src/waking_spsc.rs` | Network/application/service P1 ↔ P1 | Ordinary AtomicWakers for consumer and producer; both endpoints currently run on P1. The generic queue must not be composed across priorities with an upward wake. |
| `portable/cooperate/src/connected_tasks.rs` | Parent/worker tasks on the composition's executor | Embassy Signal under the supplied RawMutex. No hardware P1 → P2 composition uses these cells. A composition must not introduce an upward wake through them. |
| `protocols/ieee80211/runtime/src/monitor/{capture,injection}.rs` | Monitor capture/injection producer → consumer on the composition's executor | `oer_memory::WakingSpscQueue` (ordinary AtomicWakers) and a `ClaimCell` binding; no lock. A composition must not compose them across priorities with an upward wake. |
| `protocols/hil/agent/src/console.rs`; HIL request/console Channels | Synchronous callers only queue; the sole async console and dispatch run on P1 | Embassy Mutex/Channel/Watch use CriticalSectionRawMutex, which masks the hart, including P3, during a P1 operation. Logger waits descend to P1; this remains a checked deviation from the blocking critical-section restriction. |
| `chips/esp32s31/ieee80211/driver/src/rx/returns.rs`, Wi-Fi MAC/power interrupt consumers | Held-frame return P1 → P2; MAC/power P3 → P2 | `IrqSignal::raise` ORs facts and pends P2 without a waker call. Registration of the polling task's handle and its wake occur only in the bound P2 executor. |
| `chips/esp32s31/bluetooth/driver/src/interrupt/{halves,wake}.rs`, `modem_lp_timer_queue.rs` | Scheduler request P2 → doorbell P3; scheduler/timer reply P3 → P2 | P2 rings the hardware doorbell; P3 owns scheduler registers and raises typed IrqSignals back. No task waker is invoked on P3. |
| `chips/espressif/ieee802154-engine` and `protocols/ieee802154/{port,mac}` | Engine events in P3; MAC state advanced by its caller | Synchronous callbacks/polling, no atomic-waker task cell. The model's RefCell wakers belong to its single-threaded host world. |
| `chips/espressif/executor/src/time_driver.rs`, `timer_queue.rs` | Timer alarm P3/P1 → owning executor P2/P1 | Alarm latches/pends; the executor services its own timer queue. Expired timers and foreign-context rejection call the task waker directly without an AtomicWaker or a lock held by a lower-priority producer. |
| `chips/espressif/interrupt-table/src/signal.rs` | Higher handler (`FromHandler`), lower task (`FromTask`), or same-level `Exempt(ExecutorLevel)` handler (`FromExecutorLevel`) → bound executor on the same hart | Shared state is an atomic fact word. A same-level handler cannot preempt the executor's round; it publishes facts and pends the next round. Only the receiving executor touches its task handle (`SignalExecutor::Task`, woken by a direct call, no `Waker`); the raising context never owns it, and a waker that names no task of the executor is refused. |
| `protocols/radio/port/src/pool/fifo.rs`, `portable/embassy/net/src/datapath/memory.rs`, BLE/Wi-Fi models, `portable/time-virtual` | Parts of one task or one host model world | RefCell/Option or MultiWakerRegistration, not Sync. No shared cell crosses execution levels. |
| Cooperative `cx.waker().wake_by_ref()` in driver/HIL/probe steps | Task → itself | Direct executor waker, no shared registration held during the callback. |

The Embassy network adapter's TX metadata context is an `ExecutionContext`;
HIL select `ThreadContext<0>`, their existing P1 executor. A refused
context or nested borrow returns `AccessRefused` without consuming a queued
owner; TX publication refusal returns its input packet. The network device,
radio runner, RX publisher and link controller cannot be copied or sent to
another thread. The RX queue's ordinary wakers and the pool waiter remain in
that same P1 composition; no new cross-priority distribution is admitted.

Direct `embassy-sync` dependencies remain in `portable/cooperate`,
`protocols/hil/agent` and the three HIL images. The
layer test checks those exact edges against its shrinking deviation list;
the absence of priority preemption does not admit a new mutex-backed path.

## Static check

`cargo test -p oer-test-layers` inventories raw AtomicWaker spellings in
production source under `chips/`, `portable/`, `protocols/` and `firmware/`.
Only reviewed files with their recorded occurrence counts are admitted; new
files or additional uses require review. The upward fields are separately
checked for `PriorityWaker`. This is a grep gate, not priority inference:
a change of executor placement, a type alias or a renamed dependency needs the
composition review described above. It does not infer priorities from a
Rust type name or audit the source of external dependencies.
