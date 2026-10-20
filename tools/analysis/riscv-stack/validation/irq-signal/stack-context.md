# Checked stack-address contexts

The CPU0 numerical context reaches rx-poll on the current w1 ELF. The
function-wide context remains unknown because the IRQ poll invocation also
applies and has no numerical trap-arrival context. The actual constructor
application still returns **`Err(DestinationBounds)`**. The validator supplies
no numerical interval or additional trusted contract.

Fresh base: main `dd942640`, including #222 and its `6ed51a2b` Installed/Entry
fix. The [split-dispatch report](../indirect-cfg/README.md) and its
[machine proof](../indirect-cfg/proof.json) record the fresh ELF hash, full
indirect target set, common CFG closure and checked CPU0 propagation.
The constructor call is `0x5006c66e` in rx-poll `0x5006bb8a`: destination is
entry-sp minus 280, source is entry-a0 plus `0x554`, length is 68, and the
required upper bound is `D <= 0xffffffbb`.

| Checked fact | Machine provenance / result |
| --- | --- |
| Task storage | `0x500aba40..0x500dba40`, 196608 bytes, existing linker symbols |
| IRQ0 storage | `0x2f0074c0..0x2f00f4c0`, 32768 bytes, existing linker symbols |
| Installed sp / mscratch | `0x500dba40` / `0x2f00f4c0`, all normal returns of `_runtime_stack_bootstrap` at `0x2f000840` |
| Root connection | Installer call `0x5001761e`; checked continuation and tail `0x5001764e` to `runtime_main` at `0x50074e2e` |
| Main CFG | Jump `0x50065a40` in `0x5006553a`: all 30 table targets proved; ordinary shared closure check passes |
| CPU0 executor entry | Main call `0x50069490` proves entry sp `0x500db0f0` |
| CPU0 poll invocation | `0x50019c1c`, sp = executor entry-sp minus 80; rx-poll entry sp `0x500db0a0` with `ExecutorInvariant` |
| IRQ poll invocation | `0x2f003490`, existing `TaskPolls` resolution; numerical trap-arrival pair unproved |
| Context join | Both poll invocations remain applicable; merging an unknown IRQ context returns `None` |
| Real application | `Err(DestinationBounds)`; no caller memory value is installed |

An allocated task stack does not prove the interrupted task's numerical sp.
IRQ prologue and nesting checks measure relative depth, not trap-arrival
sp/mscratch values. Neither storage capacity nor a partial interrupt budget
establishes that pair or its nonwrapping propagation through nesting.

The common ELF storage reader returns start, end and capacity. Placement
accepts empty sections; stack budgets require nonempty storage. Neither says
where current sp lies. Canonical return-register inputs and the actual final
`csrw mscratch` prove the installer's pointer values on every normal return;
an entry bypass is rejected. The checked continuation connects this proof to
the actual startup call and root transfer.

That continuation context is explicitly `Installed`: it proves only live sp,
provided no subsequent sp write occurs. It cannot bound `EntryStack` pointers
in the installing function. Saved pre-installation pointers are not rebased.
At a checked call/tail, live sp becomes the callee's `Entry` context, without
rebasing saved registers or arguments. The regression saving old sp =
`0xffffffd0`, installing sp = 1024, then copying to saved sp minus 16 still
rejects destination `0xffffffc0` with `DestinationBounds`.

The opt-in stack decoder uses the common evaluator for `fence.i` and `wfi`.
Waits preserve integer registers, forget remembered words and retain an
explicit memory gap. They cannot establish source preservation. The ordinary
CFG builder consumes machine-proved jump targets and its shared closure check
still rejects other unknown edges and decoding gaps.

`StackContext` has no arbitrary-interval constructor. Checked factories derive
intervals and alignment residues from machine inputs and propagate signed
stack coordinates through calls/tails with nonwrapping RV32 checks. Joins
unite intervals, residues, origins and assumptions; any missing invocation
context stays unknown. Task-poll facts retain `ExecutorInvariant`. One selected
invocation cannot establish a function-wide interval.

Preparation and reapplication require the current context independently of
the cache, the same immutable image capture, unchanged caller bytes, matching
target/ABI and live arguments, and the current run's shared CFG closure.
Unknown or wrapping transitions cannot supply descendant intervals. Tests
cover stale code, context and image, saved old pointers, restored old stacks,
post-installation aliases, conflicting roots and refused admission.

The word certificate still has `source_unchanged_before_load = false`.
Destination bounds and publisher/source equality remain separate obligations.
Even a successful bounds discharge would establish load-time word provenance,
not equality with a particular publisher.

Reproduction and current verification are in the
[split-dispatch report](../indirect-cfg/README.md). Its
[full bundle comparison](../indirect-cfg/comparison.json) checks all 14 machine
files, eight reports and four callback audits against clean main. Machine
bytes and full audits are identical; only four report resource rows change.
The existing positive numerical fixtures and Installed/Entry/stale-proof
regressions remain enabled. The new validator checks both actual poll inputs,
the closed main CFG and the CPU0 interval, while rejecting forged targets,
omitted IRQ input and forged successful applications.
