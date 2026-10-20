# Checked stack-address contexts

The numerical application machinery discharges `DestinationBounds` in the
positive machine fixtures. **The requested real-call discharge is still
unproved.** Its actual application remains `Err(DestinationBounds)`; the
validator supplies no interval or extra trusted contract to remove it.

Fresh base: main `2966eb81`, including #207. The current control call is
`0x5006c62a` in rx-poll `0x5006bb46`: destination is entry-sp minus 280,
source is entry-a0 plus `0x554`, length is 68, and the required upper bound
is `D <= 0xffffffbb`.

| Checked fact | Machine provenance / result |
| --- | --- |
| Task storage | `0x500ab9c0..0x500db9c0`, 196608 bytes, existing linker symbols |
| IRQ0 storage | `0x2f0074c0..0x2f00f4c0`, 32768 bytes, existing linker symbols |
| Installed sp / mscratch | `0x500db9c0` / `0x2f00f4c0`, all normal returns of `_runtime_stack_bootstrap` at `0x2f000840` |
| Root connection | Installer call at `0x5001761e`; checked continuation and tail at `0x5001764e` to `runtime_main` at `0x50074dea` |
| CPU0 poll invocation | `0x50019c1c`, sp = executor entry-sp minus 80; numerical entry unknown because main has the open indirect edge at `0x500659fc` |
| IRQ poll invocation | `0x2f003490`, existing `TaskPolls` resolution; numerical trap-arrival pair unproved |
| Trap / nesting | All 47 vector prologues and exception checked; frame 80; hart 0 lower level 1 is fully bounded at 576 bytes, total remains `15664 + ?` |
| Real application | `Err(DestinationBounds)`; no caller memory value is installed |

Both poll invocations remain admissible under the existing explicit
`ExecutorInvariant`. An allocated PSRAM task stack does not prove that an
interrupted task's current sp still lies there. The IRQ checker uses the
relative ordering of the two pointer values; this new numerical application
does not assume that ordering or nonwrapping from storage placement or the
partial total. Closing that trap-arrival link and the main CFG edge remains
necessary before the real destination can be admitted.

The common ELF storage reader returns start, end and capacity. Placement
accepts empty sections; stack budgets require nonempty storage. Neither
result says where the current sp is. Canonical return-register facts and the
actual final `csrw mscratch` prove the stack installer's two pointer values
on every normal return. An entry bypass of that CSR is rejected. The checked
continuation binds the installer to its actual startup call and root transfer.
The continuation context is explicitly `Installed`: it proves only the live
sp, provided there are no subsequent sp writes. It cannot bound any
`EntryStack`-derived pointer in the installing function. The evaluator does
not distinguish pointers saved before and after a call that changes sp, so
both are conservatively refused there. At a checked call/tail, the live sp
becomes the callee's `Entry` context; saved registers and arguments are not
rebased. The diagnostic prints both bases and their machine origins.

The opt-in stack decoder uses the common evaluator and models `fence.i` as preserving integer
registers and data words. It models `wfi` as preserving integer registers
while discarding remembered memory words and retaining a `WaitMemory` gap;
waits cannot establish source preservation. A single unresolved terminal edge may be refined
only when the existing evaluator proves its target outside the function and
every other graph edge/gap closes; the ordinary CFG is then rebuilt and its
shared closure result rechecked.

`StackContext` has no arbitrary-interval constructor. It derives numerical
values from canonical machine inputs, then propagates signed relative stack
coordinates through calls/tails with checked RV32 arithmetic. It records the
invocation origins, owner, point and image capture. Joins unite all intervals
and modulo-four residues; any missing context remains unknown. One selected
invocation never establishes a function-wide interval. Existing task-poll
resolutions retain `ExecutorInvariant`, including in returned `CallWord`
expressions; moving the existing resolution types to the common model does
not change their assumptions or contracts.

Preparation checks current arguments, local SP changes and shared CFG closure.
Reapplication requires the current context independently of the cache, the
same immutable image capture, unchanged caller bytes, matching target/ABI and
all required live arguments, plus the current run's shared closure result.
A new image capture rejects the old context even if its local code/arguments
are identical. Code can establish a new absolute stack value; an unknown or
wrapping transition cannot lend a numerical interval to a descendant.

The current machine evidence and remaining links are in
[current-chain.json](current-chain.json). Storage capacities and `partial + ?`
interrupt budgets are never converted into numerical roots. The diagnostic
reuses the existing trap-entry and per-hart/per-level nesting checks and
prints their completeness separately from address facts. In particular, it
still needs the numerical interrupted sp/mscratch pair and its nonwrapping
propagation through the applicable nesting contexts. The CPU0 ancestor also
has a specific unresolved local indirect edge. The CPU0 executor's relative
SP is proved as entry-sp minus 80, including its wait loop, but its numerical
entry still depends on that open ancestor. Both task-poll entry contexts remain possible.

The word proof still has `source_unchanged_before_load = false`. Even a
successful destination-bounds discharge would establish load-time word
provenance, not equality with a particular publisher. Source preservation
remains a separate obligation.

Regressions cover changed setup pointers, frame offsets, machine stack
switches, different roots, conflicting incoming SP values, missing contexts,
unknown jumps, overflow, alignment, stale code/context/image, explicit
executor assumptions, cancellation and refused admission. The model input
`D = 0xffffffc0` remains rejected.

The review regression saves old sp in s0, installs sp = 1024, then copies to
s0 minus 16. With old sp = `0xffffffd0`, that destination is `0xffffffc0`.
It failed on `fc11a598`, which incorrectly returned `CallWord`; it now returns
`DestinationBounds`. Additional cases exercise saved registers, discarded
spills, post-installation aliases, cached proof reuse, pre-installation frame
offsets, call/tail entry propagation and restoring old sp. Restoring sp after
installation rejects propagation even when the canonical old coordinate is
unchanged. A freshly established callee entry still admits its own stack
destination proof.

Reproduction uses the existing image capture and typed DWARF; no additional
ELF/DWARF walk supplies analysis facts:

```sh
cargo fw build w0
cargo fw build w1
cargo build --locked --profile tools -p oer-analysis-riscv-stack --examples
python3 tools/analysis/riscv-stack/validation/irq-signal/explain.py current-chain \
  target/fw/w1/runtime.elf \
  "$HOME/.cache/open-esp-radio/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf" \
  /tmp/oer-stack-context-current
python3 tools/analysis/riscv-stack/validation/irq-signal/compare-stack-context.py \
  /tmp/oer-stack-destination/before /tmp/oer-stack-destination/after \
  --output /tmp/oer-stack-destination/stack-context-comparison.json
```

Capture both revisions in the same checkout path, copying `target/fw/$app/.`
into each `$snapshot/$app/` after its build, and run all four callback audits
as described in the [shared capture instructions](../callback-address-audit.md#reproduction).
The comparator checks all 14 machine files, all eight complete reports and
all four full audits, retaining complete audit diffs. The opt-in profile leaves all four full callback audits unchanged, including
individual unknown reasons, proof stopping PCs and candidate sets.

Validation on that base: tests for all 50 affected host packages/consumers,
including 228 stack tests, all-target clippy under the workspace warning
policy, formatting and diff checks pass. Both w0/w1 builds pass. All 14
machine files and four full audits are byte-identical. All eight reports
have identical semantics; only the `analysis:` resource row changes in the
four runtime/bootstrap stack reports. Canonical image/return input records
and shared resolution bookkeeping account for that work; no bound, coverage
review, unresolved site or assumption changes. See the full
[comparison](stack-context-comparison.json).

Two independent current-chain captures have identical eight streams and
JSON. The [bad-fact replay](check-stack-context.py) passes the positive replay
and rejects all ten injected changes, including rebased entry pointers,
omitted contexts, omitted CFG evidence, forged intervals and forged successful
applications; its
[results](current-chain-checks.json) supplement the Rust factory regressions.
