# `cargo fw`

Builds the staged images of `firmware/apps`, writes them into a stand board's
flash and runs them to a verdict. `boot-smoke` and `radio-root` are the
application images. HIL and probes are selected with paths relative to
`firmware/apps`, such as `../hil/esp32s31` or `../probes/esp32s31-radio`.

```console
cargo fw sizes <base-output-directory> <head-output-directory>
cargo fw sizes <base-image> <new-composition> --add-base <second-base-image>
cargo fw decode-console <console.raw.txt>
cargo fw build <image>
cargo fw build <probe-image> --bin <binary> --diagnostic
cargo fw flash <image> --board <id|MAC>
cargo fw run <image> --board <id|MAC> [--timeout <duration>]
cargo fw monitor --board <id|MAC> [--start]
cargo fw boards
cargo fw power-cycle --board <id|MAC>
```

## Build

`build` runs the image pipeline (`build(ImageSpec)`, [README.md](README.md))
on the image's package: the stage-two runtime for
`riscv32imafc-unknown-none-elf` with the `firmware` profile, packed and
embedded in the bootstrap, and the flash contents encoded for the `[flash]`
map of `chips/esp32s31/platform/chip.toml`. The bundle lands in
`target/fw/<image>/`, described by its `bundle.json`; `flash` and `run`
write its flash files in `ImageBundle::segments` order, checked against the
digests the manifest records.

Every image builds with the compiler's frame records (`-Z emit-stack-sizes`),
implicit moves above the stack policy's `max_move_bytes` reported (`-W
large-assignments`), and through the image linker
(`oer-tool-image-linker` of `tools/image/linker`), which refuses bytes in an input section
the boot zeroes. The stack checks then bound the runtime's task and
interrupt stacks and the bootstrap's stack under
`chips/esp32s31/platform/stack.toml` (`runtime-stack.txt`,
`interrupt-stack.txt`, `bootstrap-stack.txt`), and the placement check holds
the runtime to the chip's `[placement]` contract (`placement.txt`).

| Mode | Required checks | Reported checks |
| --- | --- | --- |
| `build` | Interrupt stacks (`Enforced::Only({Interrupts})`) | Frames, panic budgets, task stacks, bootstrap stack and placement |
| `run` | None (`Enforced::Reported`) | All requested checks |
| `build --diagnostic` (only `firmware/probes` packages) | None (`Enforced::Reported`, as already used by `run`) | All requested checks |
| `flash` | Uses the existing bundle without rerunning checks | Uses its recorded results |

An interrupt stack without a proven bound (`partial + ?`), above its budget,
or dependent on an assumption outside `ADMITTED` fails `build`, and so CI.
A bound conditional only on the admitted executor invariant passes with a
warning. Images declaring `task-stack = "proven"` also reject unresolved task
paths and retain their task/bootstrap stack gate. Frame and panic limits are
reports under every policy: `build` prints runtime/bootstrap frame excesses
and panic counts by layer and hardware context, with their sites. Missing or
malformed panic-budget declarations remain configuration errors. The goal
of zero runtime panics and the ban on new panic sites remain code rules.
Compiler `panic_const_async_fn_resumed` sites have a separate row outside
the budgets and target of 0, based on the executor's runtime `DONE` check
(not a type guarantee) and `.await` never polling a completed child again.
A reported check that does not pass prints as a warning containing
`reported only`. Stand `run` builds report every check; `flash` writes an
already built bundle.

`build --bin BINARY` selects a companion binary in the image package.
`--diagnostic` is an explicit build option restricted to `firmware/probes`
packages. It uses the same `Enforced::Reported` policy as `cargo fw run`: requested
product checks still run and retain their results, but a failed reported check
does not reject the diagnostic bundle. It changes no panic budget or stack/frame
limit. Compiler errors and the mandatory bootstrap-placement audit below remain
fatal. A diagnostic bundle is not product qualification.

The bootstrap's cache-off code/constant placement is mandatory in every staged
build, including `run`: all code and ordinary constants must be in internal
SRAM before the image can be encoded. It is never a reported-only failure.
`bootstrap-placement.txt` records this audit of the actual linked ELF.

## Boards

`boot-smoke` and `radio-root` send typed boot diagnostics, including the
previous panic, as `OER-DIAG:01…` lines. `run` and `monitor` decode these on
the host; the rendered `OER-PANIC previous …`, handoff, clock and
`OER-RESULT:` lines retain their earlier fields and order. The interrupt
flag is explicit even if a panic has no interrupted PC. Blank separating
lines may appear. A plain serial terminal now shows encoded records.
`run` saves the original encoded lines in `console.raw.txt` beside its
rendered `log.txt` (line endings are normalized to LF).
`decode-console PATH` renders that capture without opening a board and
rejects a diagnostic without its terminating newline at EOF.

The [codec](../../../portable/diagnostics/README.md) checks the version,
record type, lengths, UTF-8 and CRC. The host checks sequence from zero,
including the final verdict, so missing, duplicated or reordered records
cannot pass a run. A ROM reset line starts a new sequence. `monitor`
reports decode errors and keeps reading; `run` and offline replay fail.
HIL images retain their existing envelopes and console path.

Boards and hubs come from the owner's stand file,
`~/.config/open-esp-radio/stand.toml` (or `OER_STAND_FILE`, mode 0600),
which `oer-lab-router` reads ([tools/lab/router/README.md](../../lab/router/README.md): its
schema, what each command reads of it, which values are secret). Attached
boards are found by the LAB service in sysfs as Espressif USB Serial/JTAG
devices (`303a:1001`), whose serial number is the chip's MAC. Only ESP32-S31
boards are reset or written; other chips are only listed.

Every board command leases the board and the air (shared) from the LAB
service ([tools/lab/service/README.md](../../lab/service/README.md)), which
runs each operation on the board: the board is used by one lease at a time,
for every checkout. When the command ends it finishes the lease: the service
parks the board in its ROM's download mode, so an image runs only while its
command does, and a board the service could not park fails the command.

- `flash` writes the segments through one espflash connection, skipping each
  one whose flash contents already match (MD5), the OTA selection last, and
  starts the image; once the command ends the board waits in its ROM again.
- `run` builds and writes the image, leaves the chip in the flasher stub,
  opens the console and resets the chip into the image with an RTS pulse, then
  reads until `OER-RESULT: pass …` (exit 0), `OER-RESULT: fail …` (1) or the
  timeout (2): `--timeout`, or 30 s by default. The console goes to `target/fw/runs/<time>-<image>/log.txt`.
- `monitor` opens the console without a reset: RTS is released before DTR.
  Between leases a board waits in its ROM's download mode; `--start` resets
  it into its flashed image first.
- `boards` lists each attached board, as the LAB service finds it, with the
  hub port found on the host and whether that port may be power-cycled (or
  why not).
- `power-cycle` switches the board's hub port off and on with `uhubctl`. The
  port is found on the host and switched only when the board is the one
  device below it and the stand file's hub policy allows it (the stand file,
  in `tools/lab/router`). The board must leave USB while the port is off, or the
  port "did not cut power"; then it waits for the board to return.

## Sizes

`cargo fw sizes` compares `sizes.json` files without building: `same`/`report`
exit 0, unreadable input or incompatible provenance exit 1. See the [summary format and
owner thresholds](../build/README.md#size-summaries-and-verdicts).

With `--add-base`, each directory names one `sizes.json` bundle. This explicit
composition mode permits different image recipes and compares each SRAM section
and allocated stack against the sum of the baselines. Compiler, chip, target,
profile and compiler flags must still agree. The table lists each original
image, the allocation budget and the new composition; type/frame inventories
remain reports. No input summary or provenance is rewritten.

`cargo fw sizes-refresh INPUT OUTPUT --source-root CONFIGURATION` reanalyses
saved ELF bundles into a separate schema-2 summary tree without compiling or
linking. Inputs remain read-only; the source root must supply the original
image configuration. See the [refresh workflow](../build/README.md#pr-size-table-for-an-agent).
