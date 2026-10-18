# Sampling core 1's PC and SP

A profiling and hang-diagnosis technique: core 0 reads core 1's current PC
and SP from the debug-assist PC/SP record, without an interrupt line and
without interrupting core 1.

## Registers

In the ESP32-S31 SVD the record sits in the debug-assist block, grouped as
`BUS_MONITOR` at `0x2d00_2000` (field descriptions name it `ASSIST_DEBUG`).
SOURCE: `esp32s31/svd/esp32s31.base.svd` of the esp-pacs revision the
workspace pins.

| Register | Offset | Content |
| --- | --- | --- |
| `CORE_1_RCD_EN` | `0xcc` | `CORE_1_RCD_RECORDEN` (bit 0): record PC and SP in real time; `CORE_1_RCD_PDEBUGEN` (bit 1): the CPU outputs its PC |
| `CORE_1_RCD_PDEBUGPC` | `0xd0` | core 1's PC, read-only |
| `CORE_1_RCD_PDEBUGSP` | `0xd4` | core 1's SP, read-only |

Core 0 has the same triple at `0x44`/`0x48`/`0x4c` (`CORE_0_RCD_*`). The SVD
describes the PC field as "the PC value at HP CPU reset"; whether it follows
the running core live is not measured.

Not yet in this repository's register model: `registers/esp32s31` has no
debug-assist block. Use goes through the typed PAC, so the block is added to
`registers/` and generated before any code reads it.

## How the predecessor sampled

- **Mechanism in use (up to 2026-10-16).** Core 0's hang watchdog and PC
  profiler asked core 1 for a sample through a FROM_CPU software interrupt
  (one line shared by both requesters, a pending flag per requester), handled
  on core 1 at the highest level. Core 1's handler recorded its interrupted
  `mepc` and `ra` (the profiler) or `mepc`, `ra`, `sp`, `mcause`, `mstatus`
  (the hang watchdog).
- **Profiler.** SYSTIMER alarm 1 interrupts core 0 at the highest level every
  period; the handler records core 0's pair and requests core 1's. Period
  200 µs to 1 s, host-armed; 8192 samples per hart (16 s at 2 ms). Each sample
  costs two highest-level interrupts.
- **Hang watchdog.** SYSTIMER alarm 0 checks both executors' heartbeats every
  250 ms; after 3 s without progress it takes 16 further samples of the
  stalled hart, writes a post-mortem and resets.
- **Caveat of the interrupt method.** A core 1 spinning with interrupts masked
  never takes the request; it reports "not responding". The interrupt also
  takes a FROM_CPU line that the radio needs.

## The record-based method (decided, not run)

Predecessor decision, 2026-10-16: core 0 reads `CORE_1_RCD_PDEBUGPC` and
`CORE_1_RCD_PDEBUGSP` after enabling `CORE_1_RCD_EN`. No line or interrupt is
used, core 1's timing is undisturbed, and a core 1 hung with interrupts
masked is still sampled. The SP replaces the second value of the old sample
pair (`ra`), so sample formats carry PC and SP.

Required HIL check before use: the record follows core 1 (a) in normal
operation, (b) while core 1 is in an interrupt handler, (c) while core 1
spins with interrupts disabled. If any fails, the alternative is core 1
sampling itself on a free timer group timer into a single-writer snapshot
(staleness set by the period; a masked core 1 leaves its last snapshot).

## Results

None recorded: the record-based method was never implemented or run in the
predecessor. A related idea, the debug-assist stack-pointer monitor (SP bounds
and the PC that crossed them) set and read over JTAG, was also only proposed.
