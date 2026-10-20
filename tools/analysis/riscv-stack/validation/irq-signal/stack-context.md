# Checked stack-address contexts

The numerical application machinery discharges `DestinationBounds` in the
positive machine fixtures. **The requested real-call discharge is still
unproved.** Its actual application remains `Err(DestinationBounds)`; the
validator supplies no interval or extra trusted contract to remove it.

The common ELF storage reader returns start, end and capacity. Placement
accepts empty sections; stack budgets require nonempty storage. Neither
result says where the current sp is. Canonical return-register facts and the
actual final `csrw mscratch` prove the stack installer's two pointer values
on every normal return. An entry bypass of that CSR is rejected. The checked
continuation binds the installer to its actual startup call and root transfer.
The common decoder/evaluator now models `fence.i` as preserving integer
registers and data words. A single unresolved terminal edge may be refined
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
has a specific unresolved local indirect edge; its canonical SP at the
executor call is unknown. Both task-poll entry contexts remain possible.

The word proof still has `source_unchanged_before_load = false`. Even a
successful destination-bounds discharge would establish load-time word
provenance, not equality with a particular publisher. Source preservation
remains a separate obligation.

Regressions cover changed setup pointers, frame offsets, machine stack
switches, different roots, conflicting incoming SP values, missing contexts,
unknown jumps, overflow, alignment, stale code/context/image, explicit
executor assumptions, cancellation and refused admission. The model input
`D = 0xffffffc0` remains rejected.

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
all four full audits, retaining complete audit diffs. The only expected audit
classification change is still-unknown `ControlFlow` becoming `UnknownWrite`
when the analysis can continue past `fence.i`; candidate sets must agree.
