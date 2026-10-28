# Espressif staged runtime

`oer-espressif-staged-runtime` owns stage-two entry, SRAM initialization,
traps, vector tables, hart-local critical sections, interrupt stacks and
retained panic/fatal reporting. Chip access uses the platform port selected
by the image's `entry!` parameter. Build-time chip facts generate each hart's
vectors, stack symbols and default handlers.

`_runtime_start` initializes application data and SRAM interrupt/DMA sections,
selects the boot hart's PSRAM task stack and calls the image's `runtime_main`.
`entry!` supplies that entry and the image's single panic handler. The image
takes the HAL reset root's runtime partition and calls `Handoff::adopt` with
its interrupt table, controller, board mapping adapter and previous-fault
reporter. Adoption writes no PSRAM, cache or MMU register.

Adoption installs the SRAM interrupt stack and vectors, silences sources and
disables lines through the interrupt controller. Geometry and level-bit
mismatches return `BootError`. The returned `Adopted` lends its router and
affine hart token for executor setup. `enable_interrupts` checks vectors,
routes and lines, consumes pre-admission authority and returns the router
with that same hart. [Interrupt-table](../interrupt-table/README.md) owns
binding and its `Bound`/`Silent` event authority.

Stage-two code, ordinary data and task stacks use PSRAM. Interrupt entries,
hot code, critical state, DMA and interrupt stacks use internal SRAM; image
checks enforce placement. Each interrupt stack is painted on first install.
`stacks::current_hart_interrupt_stack_free_bytes` accepts only initialized
thread-mode calls, masks local interrupts for the bounded scan and restores
their state. Observed writes do not establish maximum possible depth.

The panic handler records bounded binary details in retained memory without
formatting. Interrupt-context records omit text and location metadata that
may be cached; task-context records can retain them. An optional hook receives
`PanicDetails` after capture with the local hart masked and the others stopped,
or the stop failure. Hooks and their callees/data stay in SRAM or retained
memory and take no locks. The image resets. On the next boot, `panic::take_previous` takes and clears
the panic record for the image to report; named setup faults are retained
when console reporting is refused. `retained_static!` provides
single-holder access without waiting.

Chip composition owns PHY and Wi-Fi fault records (`oer-esp32s31-compose`).
The runtime's generic `fatal::reset_recording` performs their final reset;
protocol cores do not acquire that policy. The
[bootstrap](../staged-bootstrap/README.md) owns cold initialization and
[staged layout](../staged-layout/README.md) owns the image boundary.
