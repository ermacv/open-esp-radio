# HIL stack observations

## IRQ stack evidence

The shared platform paints dedicated SRAM IRQ stacks before interrupt admission.
No image samples them.

The [stack policy](../../../chips/esp32s31/platform/stack.toml) sets the interrupt stacks' `minimum_free_bytes`; its current
reserve is an engineering guard, not a qualified nesting bound. A sampling
request that cannot complete fails explicitly. Read the
[wire contract](../../protocol/diagnostics.md#stack-measurements) for absence semantics
and the instrumentation's effect on interrupt latency.
