# IRQ callback provenance on w0/w1

Baseline: main `b88fe906`, including #172. Both unconfigured images were built
before implementation with the pinned toolchain in this checkout. Each completed
bundle, including its ELF and four reports, was copied to
`/tmp/oer-irq-signal/before/<app>`. The baseline diagnostic executable is saved
separately. Final bundles, full diagnostics, per-function common-engine records
and check logs are retained under `/tmp/oer-irq-signal/`.

## Fresh IRQ registry

[registry.tsv](registry.tsv) lists every distinct unresolved address in the
baseline **firmware gate's** interrupt report, with its function, hart/level,
executor context, pointer source, callback class and before/after barrier.
[registry.json](registry.json) adds full inline chains, symbolic entry-register
or stack/load sources, concrete common-engine graph barriers, and transported
load evidence. Names classify diagnostic rows only; they never select targets.

| ELF | Distinct unresolved IRQ PCs | Level 2 / P2 | Level 3 handlers | Classes |
| --- | ---: | ---: | ---: | --- |
| w0 runtime | 44 | 44 | 0 | 35 PHY dyn Clock, 9 platform clock release |
| w1 runtime | 61 | 58 | 3 | 35 PHY dyn Clock, 12 clock release, 7 IrqSignal.running, 3 IrqSignal.pend, 3 external RX release, 1 waker |

There are no unresolved level-1/exception sites in these reports. The counts
are sites, not address uses, candidate edges, or every raw indirect instruction.
A callback resolved by the gate's waker/task/table facts is not an unresolved
IRQ site. Experimental local field resolutions are still outside that gate,
so its registry remains unchanged after this change. An unavailable all-path
producer proof is explicitly unknown, even when the linear sweep or a symbolic
inspection sees a load. Inspection graph barriers do not substitute for the
production graph's proof.

## Selected chain: the w1 doorbell's IrqSignal.pend

The selected handler runs at P3 and pends the P2 interrupt executor. The source
chain is:

1. [The image](../../../../../firmware/apps/w1/src/irq.rs) creates static
   `MAC_SIGNAL = IrqSignal::new(InterruptExecutor::<2>::SIGNALS)`.
   `SignalExecutor::new` initializes `pend`, `running` and the signal list.
   On this ELF, `MAC_SIGNAL` occupies `0x2f007004..0x2f007028`.
   `ImageFacts::address_uses` observations **796** and **797** prove its first
   two words contain `pend_signal::<2>` (`0x2f003222`) and
   `signal_context::<2>` (`0x2f003348`). The common callback audit proves their
   storage fields (subroutine DIEs 745432 and 745453). Observations 799/800
   similarly initialize `POWER_SIGNAL` at `0x2f007028`.
2. `rx_task` passes `&MAC_SIGNAL` to `Arena::bind` in `WifiIrqSignals`.
   [Arena construction](../../../../../chips/esp32s31/ieee80211/driver/src/rx/arena.rs)
   writes `IrqStorage::new(signals.mac)`, splits it, and stores the publisher.
   `WifiIrqLineParts::install` constructs the line with that publisher and
   installs it in `RX_LINE`. These source-level ownership transitions are
   visible, but **the analyzer does not yet prove the mutable stores and
   interprocedural constructor-to-handler object identity**. In particular,
   it cannot replace the handler's loaded signal pointer with `0x2f007004`.
3. The common engine already records this nested load chain in the handler:

   | PC | Existing expression fact | Meaning from source / DWARF |
   | --- | --- | --- |
   | `0x2f00369e` | expression 47: word load from `0x2f0174c0` | mutable `RX_LINE` pointer; exact object unknown |
   | `0x2f0036fe` | expression 61: word load through 47 | line's MAC publisher |
   | `0x2f003706` | expression 64: word load through 61 | publisher's IrqStorage |
   | `0x2f003708` | expression 65: word load through 64 | storage's IrqSignal |
   | `0x2f003712` | atomic write through signal + 12 | pending facts; remains a memory barrier |
   | `0x2f00371c` | expression 68: word load through 65 | signal.executor.pend |
   | `0x2f00371e` | call inputs a0 = 65, a1 = 68 | indirect callback call |

   These expression identities are local to this common-engine record stream.
   They describe computations, not exact mutable memory values. The normal
   target-producer analysis already proves `0x2f00371c` is the call's only
   reaching load, using the complete local CFG.
4. `read_debug(TypedLocations)` describes `a0` as `&IrqSignal` in the half-open
   interval **`[0x2f00370a, 0x2f00371c)`** (variable DIE 26808, inherited type
   DIE 2460637). At the load itself, only by-value `SignalPend` and
   `IrqPublication` aggregate locations remain; they are not pointers to
   those aggregates. Before this change, local discovery found no callback
   field at the load. The nearest useful pointer witness is `a0` at
   `0x2f00371a`.

## Mechanism and real before/after evidence

[producers::bases](../../src/producers.rs) retains earlier register values equal
to a proven callback load's address base. It walks the **existing common CFG's
single-predecessor suffix** and uses the shared RISC-V semantic decoder. A move
changes the register being followed; it does not turn equal sets of possible
sources into register equality. A merge, new definition, memory write, call,
unsupported step, missing predecessor or cycle stops transport. Unclosed graphs
supply no transported witnesses. No memory contents or callee parameters are
forwarded. Both vectors and work are metered through the existing ledgers and
`RunControl`; `FunctionFacts::load_bases` carries the result to the audit.

[Local field discovery](../../src/cha/provenance.rs) queries existing prepared
locations at those earlier PCs, with the original load displacement. Conflicting
transported field types are rejected. It neither expands DWARF intervals nor
asserts memory contents across a write. The existing executor/type assumption
still applies. The address audit and its candidate policy are unchanged.

| Fact on the selected real ELF | Before | After | Remaining boundary |
| --- | --- | --- | --- |
| Static callback initializer | exact relocated typed word at `0x2f007004` | unchanged | mutable constructor/store chain to RX_LINE |
| Nested pointer loads | symbolic loads 47 → 61 → 64 → 65 | unchanged | exact signal object unknown |
| `&IrqSignal` location | available at `0x2f00371a`, unused by load discovery | witness `a0@0x2f00371a` equals base `a0@0x2f00371c` | backward suffix stops at atomic write `0x2f003712` |
| Callback field at `0x2f00371c` | unknown | `pend`, Rust ABI, subroutine DIE 2460942 | no memory/constructor target propagation |
| Field fact reaching call `0x2f00371e` | absent | load is sole producer; `pend` type reaches the call | 362 conservative candidates, not a machine-proven singleton |

The same rule finds the `pend` field at `0x2f003890` before the wifi_mac call
`0x2f003892`, and at `0x5001bc6c` / `0x5001bd34` before two hold-cycle RX-drop
calls. Their stopping PCs are respectively `0x2f003884`, `0x5001bc64` and
`0x5001bd2c`, all atomic memory writes. The two drop calls are not IRQ-registry
rows because the gate does not reach them from its IRQ roots.

## Candidates and reports

[candidate-proofs.json](candidate-proofs.json) contains all 362 targets for each
new local field site, with an inclusion witness for each and every exclusion.
For each new site, **360 targets are retained by a concrete unknown address use**,
and two by compatible typed storage (including `pend_signal::<2>`). Six of the
368 address-taken functions are excluded: every use has nonempty proven typed
storage and **every stored/called field pair is incompatible under the common
audit's own comparison**. The JSON records all original use indexes and field
DIEs. For example, `signal_context::<2>` has only the two `running` stores,
observations 797/800, whose zero-argument signature contradicts `pend`'s one
argument. Unknown slots, casts and foreign-ABI size-only comparisons keep the
existing conservative rules. No unknown use is excluded.

| ELF | Local field sites before → after | Candidate edges before → after | Existing edges removed |
| --- | ---: | ---: | ---: |
| w0 runtime | 20 → 20 | 6845 → 6845 | 0 |
| w1 runtime | 27 → 31 | 9738 → 11186 | 0 |
| w0 bootstrap | 3 → 3 | 321 → 321 | 0 |
| w1 bootstrap | 3 → 3 | 321 → 321 | 0 |

Global field sites remain zero. Every existing per-site candidate set and every
per-function address-use audit result is identical. The experimental local
resolver now returns a type-based conservative target set on four more sites;
this does not prove the selected runtime target equals its static initializer.

[comparison.json](comparison.json) records SHA-256 values and byte equality for
**all 14 machine files** (runtime ELF/bin, bootstrap ELF, application, bootloader,
partitions and OTA data for each app), complete diffs of all eight reports, and
per-site candidate differences. All machine files are byte-identical. Placement
and interrupt reports are byte-identical; only the `analysis:` resource line
changes in each runtime/bootstrap stack report. Bounds, unresolved places,
paths, coverage, assumptions and headroom are unchanged.

| ELF | Work units before → after | Peak accounted bytes before → after |
| --- | ---: | ---: |
| w0 runtime | 77820385 → 77824272 | 156779216 → 156785256 |
| w1 runtime | 100021351 → 100025206 | 158265253 → 158272661 |
| w0 bootstrap | 15717303 → 15720768 | 16354258 → 16357378 |
| w1 bootstrap | 15716619 → 15720084 | 16468946 → 16472066 |

## Verification and reproduction

149 stack regressions pass, including positive move/branch transport and negative
definition bypass, unknown incoming pointer, overwrite, unknown alias write,
unknown call effects, equal possible sources in unequal registers, and unclosed
CFG cases. The positive regression removes transport and confirms the old hole
returns with exactly the same half-open location. Existing unknown-parameter and
PC-relative pair regressions pass. Tests and clippy pass for the 37-package
selection of stack, its reverse dependencies, six common ELF/RISC-V foundations,
and dependency-layer tests, with `oer-hil-lab/test-support` and serial tests.
Formatting passes; manifests, lockfile and dependency edges are unchanged.

Production transport consumes the common observation graph, not saved records;
there is no new ELF/DWARF traversal or private CFG. The diagnostic
`function-records` example separately captures a selected function through
`research_following_tables`, and navigates expressions, accesses, transfers and
inputs with **FunctionIndex**. It never contributes proof to the resolver.

Capture each successful `cargo fw build w0` / `cargo fw build w1` bundle in the
same checkout path under `before/<app>` or `after/<app>`. Save the baseline
`callback-audit` executable before making changes. Run that executable for the
baseline and the updated executable for the final bundles, using the same pinned
ROM and reviewed summaries, as in the [shared capture instructions](../callback-address-audit.md#reproduction).
Then run:

```sh
cargo build --locked --profile tools -p oer-analysis-riscv-stack \
  --example callback-audit --example function-records
python3 tools/analysis/riscv-stack/validation/compare-callback-audit.py \
  /tmp/oer-irq-signal/before /tmp/oer-irq-signal/after \
  --output /tmp/oer-irq-signal/comparison.json
python3 tools/analysis/riscv-stack/validation/irq-signal/explain.py \
  /tmp/oer-irq-signal/before /tmp/oer-irq-signal/after /tmp/oer-irq-signal/explained
target/tools/examples/function-records \
  /tmp/oer-irq-signal/after/w1/runtime.elf 0x2f00371e
```

`explain.py` checks every unchanged old candidate set and address-use result,
asserts all inclusion/exclusion witnesses at every new site, regenerates the
registry from report contexts, and captures symbolic sources through the common
engine. Its names/addresses only select and label diagnostic output.
