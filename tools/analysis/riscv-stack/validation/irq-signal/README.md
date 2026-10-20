# IRQ callback provenance on w0/w1

The baseline is main `b88fe906`, including #172. Fresh unconfigured w0/w1 builds,
ELFs, all eight reports and the baseline callback diagnostic are saved under
`/tmp/oer-irq-signal/before/`; the baseline executable is saved separately.

The real w1 doorbell path loads `pend` at `0x2f00371c` and calls it at
`0x2f00371e`. The common CFG and target-producer analysis already prove that
load is the call's only producer. `read_debug(TypedLocations)` describes `a0`
as `&IrqSignal` in `[0x2f00370a, 0x2f00371c)`; at the load, it only describes
by-value `SignalPend`/`IrqPublication` aggregates. The direct local resolver
therefore has no callback field at that load.

The new generic load-base fact transports register value identity backwards
through a single-predecessor suffix, following register moves. The witness
`a0` at `0x2f00371a` is equal to the load's base, so its PC-scoped type proves
the `pend` field reaches the load and call. No DWARF range is extended.
The preceding atomic write at `0x2f003712` remains an explicit barrier.
Joins, definitions, unknown instructions, calls and memory writes also stop
transport. Unclosed graphs produce no transported witnesses. Memory values
and constructor parameters are not forwarded.

The shared callback audit still owns candidate exclusion. This proof identifies
a field type, not a unique signal object or an exact target set. The new local
field resolution remains outside the firmware gate and retains unknown address
uses. Function names and numerical addresses in this document are diagnostics,
never inputs to the analyzer's rules.
