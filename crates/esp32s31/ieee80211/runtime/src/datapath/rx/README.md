# RX ownership boundary

`oer_esp32s31_ieee80211::rx` composes finite prepared/live/halted descriptor
transitions, an abstract delay and the storage profile. The local `frontier`
module reexports those contracts and supplies the Embassy timer binding.
The lower `hardware/esp32s31/driver/ieee80211/dma` crate owns descriptor publication,
cursor proofs, buffer leases and sticky arena poisoning.

## Runtime composition

| Owner or port | Responsibility |
| --- | --- |
| `StagedRxProducer` | Live ring, static DMA storage, staging pool, ordered publisher, delay, admission policy and observation |
| `StoppedReceive`, `PreparedRx`, `RxEpochResources` | Retain and return the same queue sender, pool and observation capability across role epochs |
| `StagedRxPublisher` | Standalone or paired STA/AP publication; paired routing uses the retained VIF addresses |
| `routed` | Paired STA/AP transport: the VIF-classified staged frame and its single ordered queue; `roles::concurrent::StaApRxConsumer` owns role dispatch |
| `DatapathRxService::service` | Synchronously calls chip `rx::transaction::service` and wraps the result in `ready`; physical work finishes before its returned future is polled |
| `dma/service/observation.rs` | Statically dispatched clocks, logging and telemetry hooks |

## Chip transaction

`rx::transaction` borrows the live ring, DMA storage and staging pool for one
bounded completion, admission, publication and recycle pass. Its `Admission`,
`Publisher` and value-only observations require no executor. Publication
accepts the unique `NetworkRxFrame` or returns that same lease in `Err`;
callbacks receive no ring or register authority.

Two bounded released-prefix observations retain a frozen LAST boundary.
Terminal writeback handling, exhausted-list republication, append and reload
completion stay in the same synchronous transaction. It performs no async
wait, task spawn, allocation, descriptor copy or unbounded retry.

Admission order, reserved critical credit and VIF classification precede
publication. Recycling respects the contiguous released prefix. The service
borrows three cumulative counters and updates them before the final frontier
read, including when that read fails. Optional hardware samples are guarded
before evaluation. The runtime owns the recycled-append telemetry selector
and publication timestamp; the chip transaction creates no second counter or
selector. Performance builds do not read diagnostic clocks.

A pass that has just published owners yields before discarding another valid
bulk unit for lack of staging credit. The fused post-DMA protocol phase can
then release those owners; its `StageCapacityBlocked` continuation
keeps the retained DMA tail runnable. A pass that starts blocked and publishes
nothing still applies discard/recycle admission, so a persistently blocked
consumer cannot hide critical traffic behind bulk frames. Critical frames may
use their reserved credit in either case. This changes scheduling opportunity,
not pool capacity or descriptor lifetime.

The chip function is `inline(always)` into the `inline(never)` runtime service
in `.hot.text.open_radio_rx_dma_service`. The chip crate forbids unsafe code.
Final-image placement and stack checks apply after optimization; source module
placement alone does not establish internal-SRAM execution.

## Allocation through publication

| Edge | Retained authority | Return or recovery |
| --- | --- | --- |
| Completed unit remains in frozen frontier | Radio ring/storage | Capacity refusal leaves the unit in DMA ownership; no lease exists |
| Malformed or overload-discarded unit is taken | Radio ring/storage | Deferred recycle retains the unit; dropping that owner quarantines the arena |
| Valid unit is staged | One frame owns its original allocation | Descriptor remains observed until upper ownership returns |
| Publisher accepts | Queue, then protocol/network consumer | Consumer drop releases the allocation exactly once |
| Publisher rejects | Transaction receives the same frame | The error path drops the frame and reports `Corrupt`; it creates no replacement lease |
| Released allocation follows an older retained frame | Radio storage records release | Only the longest contiguous released prefix may be rearmed |

Release does not itself rearm a descriptor, and quarantine does not prove
hardware quiescence. Same-Core0 queue endpoint constraints remain part of the
composition; admission and observation hooks do not grant cross-thread MMIO
or additional physical ownership.

## Start cancellation limitation

[`StagedRxEpoch::start`](dma/epoch.rs) replaces its stored state with
`Vacant` before awaiting the consuming `PreparedRx::start`. Cancellation
during that settle delay loses the recoverable prepared owner and its sender.
This entry point therefore does not offer cancellation-safe retry. The walker
is not enabled at that point; the limitation is not a claim of live-DMA
use-after-free. A caller cannot treat cancellation as a successful stop or
recover a reusable prepared epoch from `Vacant`.

Host tests beside the chip transaction and runtime exercise lease return,
queue saturation, deferred discard, frozen-frontier recycle and terminal-owner
recovery. They do not establish on-air behavior or hardware qualification.
