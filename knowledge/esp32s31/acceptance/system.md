# System acceptance scenarios

Predecessor HIL catalog, state of 2026-10-16. No scenario here needs a peer.

## Boot smoke (qualification)

- Checks: bootstrap, PSRAM mapping, a task stack in PSRAM, the IRQ stack in
  SRAM and runtime startup. The console shows the boot-smoke and timer PASS
  markers and no panic marker.
- Gate: within 10 s of boot.
- Hardware: pass, predecessor HIL run 2026-10-04 and many earlier runs.

## Timebase: alarm and monotonic clock agree

- Checks: alarms and the monotonic clock agree before the radio is
  initialized.
- Gate: 3 boots × 20 intervals of 100 ms (2 s expected).
  - Device elapsed time is 95–110 % of expected.
  - Host elapsed time is 95–120 % of expected.
  - Each interval is 95–150 % of its period, and no interval is early.
- Hardware: pass, predecessor HIL run 2026-10-04.

## Interrupt table enable and disable

- Checks: a software-raised interrupt reaches its handler only while it is
  enabled.
- Gate: 10 probes. Per probe, the handler runs 0 times before enable, exactly
  once while enabled and 0 times after disable, within 5 s.
- Hardware: pass, predecessor HIL run 2026-10-04.

## Inter-core call

- Checks: calls core 0 → 0, core 0 → 1 and core 1 → 0 each run on the target
  core.
- Gate: 20 calls of each kind, 5 s per call. No latency bound.
- Hardware: pass, predecessor HIL run 2026-10-04.

## Panic reset and post-mortem record

- Checks: a deliberate panic resets the chip. The console shows a ROM or
  bootloader start. The surviving panic record names the panicking source
  file at a non-zero line, hart 0, in thread context.
- Gate: the reboot comes 100 ms–15 s after the panic, with reset reason
  Software.
- Hardware: pass, predecessor HIL run 2026-10-04.

## Main watchdog: completion and four failure modes

- Checks:
  - In the complete mode, no reset occurs and the image still answers 3 s
    later.
  - In the blocked-poll, cancelled, lost-completion and late-restoration
    modes, main watchdog 1 resets the chip. A post-mortem survives, and its
    last checkpoint is the watchdog arm of that mode.
- Gate: 3 repetitions. In the failure modes the reboot comes 500 ms–12 s after
  arming; the watchdog budget is 1 s.
- Hardware: pass, predecessor HIL run 2026-10-04. The post-mortem survived 12
  of 12 resets (2026-09-28).

## Hang watchdog: stalled executor or console

- Checks: stalling the protocol executor, the network executor or the console
  causes a software reboot. A hang post-mortem then:
  - names the stall (the executor bit, or the console task);
  - holds a non-zero program counter, with the stack pointer inside the
    task-stack region;
  - shows the trace frozen at the hang and then restarted.
- Gate:
  - The reboot comes 5–25 s after arming: an executor stall is detected at
    3 s, a waiting command at 5 s, and sampling adds 4 s.
  - Reset reason Software.
- Hardware: partial, predecessor HIL run 2026-10-04. Both executor stalls were
  detected; the console-stall case failed.

## Memory copy benchmark: CPU and GDMA from SRAM and PSRAM

- Checks: every copy case completes, run as CPU copy, GDMA blocking or GDMA
  async, from SRAM or PSRAM. The counters are consistent:
  - all non-zero;
  - foreground time no larger than elapsed;
  - async mode polls at least once per iteration;
  - synchronous modes poll 0 times, with foreground time equal to elapsed.
- Gate:
  - Plain variant: sizes 64, 256, 512, 1514 and 4096 bytes × 32 iterations
    (30 cases per boot × 2 boots).
  - Batch variant: sizes 64, 512 and 1514 × batches 1, 2, 8 and 32 (72 cases
    per boot × 2 boots).
  - 15 s per case.
  - No bandwidth floor; throughput is diagnostic.
- Hardware: pass, predecessor HIL runs 2026-10-03 (batch, 144 cases) and
  2026-10-04 (plain). No bandwidth values were recorded.

## Stand recovery ladder

- Checks:
  - The board leaves USB after the image turns USB off.
  - The reset ladder clears, ending with a power cycle.
  - Download entry prints a ROM DOWNLOAD reset line.
  - An RTS reset then boots the image again.
- Gate: USB gone within 5 s; return and answer within 15 s each.
- Hardware: not confirmed.
