# Typed signal executor: production gate evidence

Baseline: `f3849aa50862f8e2d199b7b307e09cdcd90c4049` (main after #231).
Implementation: `e0f2349e`. Both sides use the pinned Rust 1.97.1 toolchain,
`firmware` profile, fat LTO and the normal `cargo fw build w0`, `w1`, `w2`
pipeline. The application configuration is the default unconfigured build on
both sides. This evidence does not describe a configured scan or a hardware run.

The before/after directories contain the original production reports:
`placement.txt`, `runtime-stack.txt`, `interrupt-stack.txt`,
`bootstrap-stack.txt`, section sizes and the relevant `llvm-nm` inventory.
`summary.json` records the ELF and runtime.bin digests and inline stacks;
`sites.tsv` lists every unresolved IRQ site. Diagnostic classifications do not
resolve edges or change the gate. Six baseline running checks and the existing
control waker lack inline DWARF source locations after LTO; their labels are
bound to the exact ELF digests in `labels.json` and supported by the retained
disassembly.

## IRQ sites and bounds

Counts are distinct unresolved PCs in the production interrupt report, grouped
by hart and level, rather than raw indirect instructions or candidate edges.
Both images' level-1 handlers and hart-1 level-1 handlers have zero unresolved
sites before and after.

| Image / hart 0 | P2 sites before → after | P3 sites before → after | P2 bound before → after | P3 bound before → after |
| --- | ---: | ---: | --- | --- |
| w0 | 3 → 3 | 0 → 0 | 14944 + ? → 14944 + ? | 576 → 576 |
| w1 | 14 → 7 | 3 → 0 | 14080 + ? → 14080 + ? | 576 + ? → 576 |
| w2 | 1 → 1 | 3 → 0 | 576 + ? → 576 + ? | 576 + ? → 576 |

All seven w1 `IrqSignal::owned` running checks and all three
`SignalPend::pend` executor callbacks disappear. Hart 0 P3 has a numerical bound
without `+ ?`. The full hart-0 bounds remain partial: w0 16544 + ?, w1 15680 + ?,
w2 2176 + ?. Hart 1 remains 1024 bytes, conditional on the executor invariant.

| Remaining w1 P2 category | Before | After | After PCs |
| --- | ---: | ---: | --- |
| Signal executor running | 7 | 0 | — |
| External RX release | 3 | 3 | 0x2f004554, 0x2f004614, 0x5009ed3a |
| PHY jump table | 3 | 3 | 0x5001d0f2, 0x5003e120, 0x5003e18c |
| Receive control AtomicWaker | 1 | 1 | 0x5005e9b2 |

**Deviation from the requested exact residue:** the stop-control waker was
already unresolved on main at `0x5005e972`, in `Receiver::run`'s
`ReceiveRunnerControl::poll_stop` / `AtomicWaker::register` path. It remains
unresolved at `0x5005e9b2`. Its call joins RawWakerVTable wake / wake_by_ref
branches; it is neither a signal executor callback nor a jump table. Thus the
residue is 3 external releases + 3 PHY jump tables + 1 existing waker, not just
the first two categories. No new unresolved IRQ site category is introduced.
w0 retains exactly its three PHY jump tables. w2 retains one Embassy raw task
poll; its three P3 signal-pend sites disappear.

## Size and placement

All sizes below are bytes. The complete table, including critical SRAM data,
is in [sizes.md](sizes.md).

| Image | Metric | Before | After | Delta |
| --- | --- | ---: | ---: | ---: |
| w0 | runtime.bin | 525777 | 525777 | 0 |
| w0 | .isr.text | 16572 | 16572 | 0 |
| w0 | .text | 460292 | 460292 | 0 |
| w0 | .bss | 9584 | 9584 | 0 |
| w1 | runtime.bin | 645601 | 645809 | +208 |
| w1 | .isr.text | 19868 | 19856 | -12 |
| w1 | .text | 556374 | 556590 | +216 |
| w1 | .bss | 59776 | 59776 | 0 |
| w2 | runtime.bin | 65536 | 65536 | 0 |
| w2 | .isr.text | 17248 | 17228 | -20 |
| w2 | .text | 31002 | 31002 | 0 |
| w2 | .bss | 224 | 224 | 0 |

There is no `.isr.text` growth: w1 shrinks by 12 bytes and w2 by 20. No placement
attribute was added or removed; `pend_signal`, `signal_context` and the
interrupt handlers retain their existing SRAM placement. Direct context calls
and typed wrappers change generated instructions, not the placement policy.
In w1, the two signal statics shrink from 36 to 24 bytes each and
`.critical.data` falls by 24 bytes. The total runtime grows by 208 bytes with
`.text` growing by 216 bytes; alignment and data account for the difference.
w0's sizes are unchanged; equal size does not imply identical file contents.
All six placement reports say `PASS`, with handlers in SRAM/ROM.

| Image | CPU0 task bound before → after | Cached handler functions before → after | Handler panic calls before → after |
| --- | --- | ---: | ---: |
| w0 | 17104 + ? → 17104 + ? | 240 → 240 | 176 → 176 |
| w1 | 23040 + ? → 23024 + ? | 269 → 269 | 206 → 207 |
| w2 | 864 + ? → 864 + ? | 0 → 0 | 6 → 7 |

The additional reported panic edge is `pend_signal::<2> -> panic_fmt`, now
reachable through a direct pend call; the old unknown callback hid that path. These counts are the gate's reachable-function counts, not total cached
section bytes. The cached-memory placement findings and partial stack findings
remain in the reports; successful build exit codes do not make these warnings
complete proofs. Bootstrap remains 736 + ? with 29 unresolved sites.

## Sharing and llvm-nm

`SignalCore` and `SignalList` have no executor parameter. Only the public
`IrqSignal<E>` context boundary and `SignalPend<E>` pend are specialized. One
private zero-sized context token covers each list traversal. No token or raw
core pointer appears in the public API.

**Symbol-count limitation:** fat LTO inlines the core and wrapper methods in the
production w1/w2 ELFs, so `llvm-nm` reports zero standalone `SignalCore` or
`IrqSignal` methods, rather than one separately named copy. It cannot prove a
literal one-emitted-core-function criterion for these ELFs. The inventories
show one `pend_signal::<2>` and one `signal_context::<2>` in each w1/w2 image,
with no other executor specialization. w0 has no retained signal code. In the
unconfigured w2 build the signal statics themselves are dead-stripped.

[host-signal-symbols.txt](host-signal-symbols.txt) additionally records the host
unit-test ELF: one non-generic set of `SignalCore::{has_pending, poll_pending,
poll_take, register, service_next, take}` is shared by different executor types.
`owned` is specialized for the four operation-local closures, independent of
executor type; raise is always inlined. No `inline(never)` or retention knob was
added to manufacture production symbols for the audit.

## Validation

[checks.json](checks.json) records each command's own exit code. All final
commands succeeded:

- `cargo test` and `cargo clippy --all-targets` for interrupt-table (host-signal),
  executor, ieee80211-mac, ieee80211-driver and bluetooth-driver.
- One RISC-V check covering those five crates, with the S31 executor feature.
- `cargo fw build w0`, `w1`, `w2` separately.
- Both S31 probes declared in the vendor catalog: radio and register probes.
- `cargo hil images check --chip esp32s31 --reaching oer-interrupt-table --jobs 2`:
  boot-smoke, system-watchdog, system-panic-reset, diagnostic-phy and
  diagnostic-usb-jtag-off: 5 passed, 0 failed, including the normal image audits.
- RISC-V Bluetooth doctests: 7 passed, including the updated generic examples.
- `cargo fmt --all --check` and `git diff --check`.

Existing behavioral tests still cover waker replacement/drop reentrancy,
coalescing and facts raised while servicing. Three focused signal tests cover
foreign service without even calling `list()`, one traversal of two signals,
and exactly one pend per new fact including deferred pend. The new
`mac/tests/ui/signal_executor.rs` case uses the existing trybuild runner and
rejects `&'static IrqSignal<A>` where `IrqSignal<B>` is expected. Interrupt-table
has no trybuild dependency; no new UI harness was introduced there.

The initial probe invocation exited 1 because a cached CLI had been compiled
from the baseline worktree; the explicit supported `--root` retry exited 0.
Both outcomes are retained in the command record. Host Clippy has existing
repository warnings; none of these commands used output text as its verdict.

## Reproduction

Keep separate before/after copies of `target/fw/{w0,w1,w2}` containing
`runtime.elf`, `runtime.bin` and the four production reports. Generate this
summary with:

```sh
python tools/analysis/riscv-stack/validation/typed-signal-executor/report.py \
  /tmp/typed-signal-evidence/before /tmp/typed-signal-evidence/after \
  tools/analysis/riscv-stack/validation/typed-signal-executor \
  --base f3849aa50862f8e2d199b7b307e09cdcd90c4049 --head e0f2349e
```

The local binaries, complete build/test/Clippy logs and original command
records remain in `/tmp/typed-signal-evidence`. The checked-in text reports and
digests are sufficient to review the tables without adding ELF binaries to Git.
The labels deliberately reject different ELF digests; re-audit source-less sites
when rebuilding in a different checkout or with another compiler configuration.
