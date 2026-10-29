# Espressif staged runtime

`oer-espressif-staged-runtime` owns stage-two entry, SRAM initialization,
traps, vector tables, hart-local critical sections, interrupt stacks and
retained panic/fatal reporting. Chip access uses the platform port selected
by the image's `entry!` parameter. Build-time chip facts generate each hart's
vectors, stack symbols and default handlers.

`_runtime_start` initializes application data and SRAM interrupt/DMA sections,
selects the boot hart's PSRAM task stack and calls the image's `runtime_main`.
`entry!` supplies that entry, the image's single panic handler and the common
CPU-exception handler. The image
passes the HAL reset root's unstarted runtime partition to `adoption::run`,
with its composition binding, interrupt table, board mapping adapter and
previous-fault reporter. The runner calls `oer-espressif-boot::adoption::initialize`
for CPU-rate/clock adoption, console/core/debug installation, runtime watchdog
arming and interrupt-owner publication. Isolated verifiers call that same driver
without linking the runtime. The HAL split performs no startup effects.

The image's ownership projection callback runs after console installation and
watchdog arming, before mapping/context adoption. It consumes the reset-cause
owner once, reports raw causes for every hart, reports image retained records,
and forwards the owners needed by the image. HIL also keeps the reset-cause
value snapshot for later boot evidence. The runner reports the runtime's named,
boot and CPU-exception records, then calls the family mapping-adoption API and
installs the interrupt context. Only a successful arm/adopt path constructs
`adoption::Runtime`; its constructor and fields are private. `into_parts`
returns the projected hardware, mapping and interrupt context together.
`prepare` exposes the initialized phase as an opaque `Prepared`; its owners stay
private until `Prepared::adopt` invokes the projection and completes adoption.
HIL reports reset/retained evidence there and emits its startup banner and
layout checks after successful adoption. `run` uses the same preparation and
completion implementation.
Adoption writes no CPU clock, PSRAM, cache or MMU register and repeats no cold boot.

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
`PanicDetails` and the winning terminal claim after capture with the local hart
masked and the others stopped, or the stop failure. Hooks and their callees/data
stay in SRAM or retained memory and take no locks. The image resets.

The exception entry captures `mcause`, `mepc` and `mtval` in the existing
96-byte frame before Rust/platform work. It commits that frame before peer stop
and optional image observations, then resets. Reentry preserves the first
committed minimum and skips the recorder. One reset-attempt gate bounds the
handler nesting; a fault in reset reaches the architectural halt and preserved
watchdog. The exported bound of three exception frames participates in IRQ stack
sums. These guarantees require intact SRAM, trap entry and local mask code;
capture and retention remain best effort.

`retained_fault` owns a boot-initialized SRAM election. `claim` returns an
exclusive terminal claim or typed `Reentered`, without waiting. The winning
claim and every slot it commits remain held until reset. The same terminal
may commit supplemental hook observations in separate slots of the same format.
A losing terminal requests reset without invoking another recorder.

Every staged fault slot uses `oer-diagnostics::fault::Record`, layout version 1:
eight numeric header words (completion, version, payload length, registry class,
domain, reason, captured hart or an unavailable sentinel, and context), 280
payload bytes and a checksum, 316 bytes in total. The largest payload is the existing HIL postmortem snapshot. Commit
publishes completion last; unsupported versions, oversized payloads and corrupt
records return typed errors. There is no previous-layout decoder.

`panic::peek_previous` reads without consuming. The diagnostics renderers clear
an ordinary panic only after successful output of its exact snapshot; named
faults and image reporters likewise clear after their complete output succeeds.
HIL acknowledges its platform panic and postmortem after the full boot-evidence
frame and CRLF are written. Output refusal retains diagnostic bytes. A new boot's
first terminal may replace an unreported older record. Retention is best effort;
checksums are not a power-loss or `ResetRtc` retention guarantee.

Chip composition owns PHY and Wi-Fi fault records (`oer-esp32s31-compose`).
The runtime's generic `fatal::reset_recording` performs their final reset;
protocol cores do not acquire that policy. The
[bootstrap](../staged-bootstrap/README.md) owns cold initialization and
[staged layout](../staged-layout/README.md) owns the image boundary.
