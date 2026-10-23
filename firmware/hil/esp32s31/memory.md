# HIL stack observations

## IRQ stack evidence

The shared platform paints dedicated SRAM IRQ stacks before interrupt admission.
No image samples them.

The image linker sizes each proven product IRQ stack from its bound plus the
chip contract's 10% margin and guard; diagnostic stacks keep the contract
maximum. IRQ stack policies no longer carry a fixed `minimum_free_bytes` reserve. A sampling
request that cannot complete fails explicitly. Read the
[wire contract](../../../protocols/hil/protocol/diagnostics.md#stack-measurements) for absence semantics
and the instrumentation's effect on interrupt latency.
