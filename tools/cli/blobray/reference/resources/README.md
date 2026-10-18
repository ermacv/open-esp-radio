# Resources and limits

Bound an operation's work, time and working memory, and read its failures.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Cooperative control and failure diagnostics

Every operation runs in its caller's process and charges one `RunControl` the
caller supplies. `blobray_application::in_process::Limits::new(max_work_units,
timeout)` is the control the CLI and the vendor scenarios use: it counts work
units against their limit on every checkpoint, checks the wall-clock deadline
every 256 checkpoints, and keeps the run position a failure reports. The CLI
exposes `--working-memory-mib` (default 256), `--timeout-secs` (default 900) and
`--max-work-units` (default 1,000,000,000); the defaults are the `DEFAULT_*`
constants of `blobray-domain`. Reaching a limit fails the operation with
`resource-limited`; no automatic limit increase or reduced scope occurs.

Work policy 1 charges one unit per visited structural record and one per block of
up to 4096 bytes for each read, hash, copy or delimiter scan. Repeated processing
is charged again. Constant-time borrowed-range access performs a zero-cost
checkpoint. Charges are admission costs before work, not CPU instruction counts.
Repeated execution with the same inputs and policy has the same work cost. ELF
string scans, explicit table loops and hashing have cooperative checkpoints.
Opaque third-party calls, allocator operations and filesystem calls are not
interruptible inside the call. A wait on the linker process polls it with
checkpoints, so an expired deadline stops the link and the linker is killed and
reaped.

The run position records the phase and the physical input/member/table/entry
ordinals and artifact digest reached. The phases include starting,
read-captured, members, elf, analyze-function and analyze-values. The position
is the last observation, not a claim about the exact failing instruction.

## Current memory boundary

`WorkingMemory` is an explicit capacity authority for one operation. Owned
`ScratchBytes` use fallible allocation and retain their reservation until drop;
Rust borrows prevent scratch references from outliving their owner. Error and
unwind paths release the same capacity. No resettable integer scratch handles or
custom global allocator are exposed. This implementation uses admitted owned
buffers rather than one mmap arena. Admission is not RSS or a claim that every
allocation goes through it; runtime, allocator bookkeeping, stacks and the
caller's own data remain outside it.

`MemberCursor` reads archive headers and names through positional `ByteSource`
reads; it does not retain all member descriptors. GNU/BSD/COFF names and the
finite AIX member index are read on demand. ELF inspection admits one
contiguous object buffer for borrowed object-crate views, plus a conservative
name workspace of twice that buffer's length and 16 KiB. An ELF that does not
fit fails explicitly; no algorithm silently switches to unbounded memory.
Sections, symbols, relocations and diagnostics are emitted to synchronous
sinks; a sink failure aborts the operation and cannot become malformed-ELF
coverage. Consumers retaining copies from streaming callbacks owe their own
capacity budget.

Capacity exhaustion returns `resource-limited` with a structured
`error.memory`: requested reservation, available and limit bytes, and the
phase, input and member of the run position. The request is an admission
amount, not an estimate of memory needed to finish. Error prose is limited to
1024 UTF-8 bytes plus a truncation marker.
