# Trace recorder

`oer-portable-trace-recorder` holds the reset-retained recorder of
[`oer-portable-trace`](../trace/README.md) events: the OETR layout 1 storage,
commit and clear, the global installation and the previous-boot policy. The
image places the retained storage and its recorder and names the clock with
`trace_clock!`. Retained words use plain atomic loads and stores; the recorder's
counters and the global pointer and mask stay in ordinary atomic RAM.

Without `cfg(oer_trace)` a record point leaves no code.
