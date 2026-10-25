# ESP32-S31 staged application boot

This platform composes the reset root (`oer-esp32s31-hal`) for
ESP32-S31-Function-CoreBoard-1. It has no
test protocol, radio role, executor or network-stack dependency.

| Component | Responsibility |
| --- | --- |
| `board` | Board facts (the PSRAM window, the flash MMU page, the antenna gain and power limits); adoption of the live PSRAM mapping |
| `bootstrap` | Flash entry, PSRAM initialization, image validation/CRC, relocation, Flash tuning and non-returning handoff |
| `runtime` | Stage-two entry, SRAM section initialization, mapping adoption, vector handoff, per-core interrupt stacks and the image's one panic entry |
| `layout` | Address map (`LAYOUT`, generated from the chip facts), the stage-two placement and the stage-two header/checksum shared with the host packer and auditor |
| `linker` (`chips/espressif/staged-layout/linker`, `chips/esp32s31/layout/linker`) | Semantic code, data, DMA and stack sections over the `layout` symbols |
| `chips/esp32s31/platform/partitions` | Application partition layout |
| `chips/esp32s31/platform/stack.toml` | Stack policy: each stack's root function, storage and reserve, the move limit and the frame-coverage review (`stack-coverage.toml`) |

The runtime owns every image's `#[panic_handler]`. It writes a bounded record
of the panic to `.rtc_fast.persistent` without formatting (the end of the
location's file path, line, column, the start of the message when it is a
static string, hart and, on an interrupt stack, the interrupted PC), so the
panic path stays a short leaf in every stack bound. An image with
`panic-hook` names its hook with `panic_hook!`, which the entry calls after
the record, in the fatal path with the hart masked and the other hart
stopped (`cores::StopTheWorld`, or the `StopError` why not), to record more
of its own state the same way; it returns. Under that stop a record may read
hart 0's routes and CLIC lines as the hardware holds them
(`interrupt_controller::fatal_view`). An
image's own reset-retained records are `retained_static!` cells, one holder
at a time without waiting, with their claim in the SRAM critical BSS. Every
image then resets, and its next boot reads and clears the record with
`panic::take_previous` and prints it.
The hook receives `panic::PanicDetails`, rather than the original `PanicInfo`.
Interrupt-context panics retain hart and interrupted PC, but omit location and
message: compiler metadata can live in cached PSRAM. Task-context panics keep
their metadata because their code and stack already require the live mapping.
The hook and all state it reads must stay in SRAM or retained memory and take
no locks. The image linker places the hook and its strict callees, together
with interrupt and exception paths, in SRAM from its analysis; these functions
have no manual placement annotations. RX hot paths use `.hot.text` for
performance.

Images are built, written and run by [`cargo fw`](../../../tools/image/cli/README.md);
no `.cargo/config.toml` adds Rust flags.

The boot sequence is ROM → ESP-IDF bootloader → Flash bootstrap → application.
The ROM image uses DIO at 80 MHz; ESP-IDF enables QIO for the application.
The image build encodes the flash contents with the
`espflash` library: the ROM-readable DIO bootloader from `espflash`'s
bootloader resources (checked), the partition table, the audited QIO
application and the `ota_0` selection, at the offsets of `chip.toml`'s
`[flash]` map. A flash writes them as raw segments through one connection,
skipping segments whose flash contents already match; the selection goes
last. It preserves NVS and other application partitions. Encoding the
bootloader in QIO with the application would change the ROM image header and
prevent this board from booting.
The bootstrap finds its payload through the flash MMU, so an application
boots from any OTA slot.
The runtime is linked separately. Its header supplies the entry, payload and
initialization ranges; the host packs the checksum before embedding it in the
bootstrap. Bootstrap copies and verifies PSRAM code before transferring control.
It does not return or carry Rust peripheral owners across the image boundary.

Stage-two assembly initializes application data and SRAM interrupt/DMA sections
before entering `runtime_main`, which the image defines with
`oer_espressif_staged_runtime::entry!`; it hands the image's `main` the one
`Handoff`. The application takes the reset root (`Root::take`, then
`into_runtime`), then
`handoff.adopt(INTERRUPT_TABLE, runtime.interrupts.controller, BootstrapPsram)`:
the board's `BootstrapPsram` implements the staged layout's
`AdoptsBootstrapMapping`, the promise that it adopts the bootstrap's mapping
without resetting or remapping PSRAM; stage two owns no PSRAM, cache or MMU
register, so adoption writes nothing. `into_runtime` arms the RTC watchdog:
every image's executor feeds it from a task
(`ArmedRtcWatchdog::feed_every`). The board does not link the runtime,
whose panic entry would otherwise reach the bootstrap through the board.
`adopt` installs the per-core SRAM interrupt stack and the runtime's vectors
(`mtvec` names the exception entry, `mtvt` the hart's table) and takes the
hart's routes and lines through `controller`, the chip's
`oer_interrupt_table::Controller`: it silences every source and disables
every line. It returns `Adopted`, whose router binds each entry and whose
`enable_interrupts` checks the vectors, routes and lines and sets global
interrupt enable. The runtime owns each hart's vector table: the word of
line `n` holds that line's entry, which switches to the SRAM stack, calls the
line's handler directly and returns through the checked exit.

Each image declares its interrupt bindings once with
`oer_espressif_staged_runtime::interrupt_table!` (interrupt part, handler,
CPU line 16 to 47, level, hart, class; see
[`oer-interrupt-table`](../../../portable/interrupt-table/README.md)): the
source comes from the part's type, the macro defines each line's handler
symbol and one token per entry, and the image hands its `INTERRUPT_TABLE` to
`Handoff::adopt`. Each 12-byte entry holds the source (`u16`), the line, the
level, the class and the hart (`u8` each) and the handler address at offset 8
(zero for an entry a `cfg` leaves out); compile-time assertions in
`portable/interrupt-table` keep that layout. Binding an entry moves its part
where only its handler reaches it, writes the line and then the route, and
returns the `Bound` proof the driver takes before it enables the
peripheral's interrupt.

Every image uses one placement: PSRAM for code, ordinary data and a 192-KiB
CPU0 task stack; SRAM for interrupt entries, hot code, critical state, DMA
storage and core 0's interrupt stack (at most 32 KiB). Core 1 runs nothing:
its task stack and its interrupt stack are an explicit 0 bytes in every image,
which the image gate proves (see [Stacks](../../../docs/memory.md#stacks)). These board sizes are not universal
chip capabilities.

Because code runs from PSRAM, the adopted mapping holds MPLL as a permanent
reference for the life of the image: the platform clock owner's count of
MPLL never reaches zero while PSRAM is in use, so a driver that releases MPLL
never powers it down under PSRAM.

## Boot state on the board

Reads on the board show what the bootloader leaves:

- The `espflash`-bundled ESP-IDF bootloader already programs PMA entry 7 as
  the locked 64 MiB NAPOT external-memory aperture at `0x5000_0000`; entry
  15 covers the flash window `0x4000_0000` (512 MiB), entries 12-14 the
  internal memories, and entry 8 is unused.
- A Flash MMU entry implements bits 0-10 and 13 of the content word: the
  page-number field the PAC publishes as 10 bits (`PADDR`, with bit 10 as
  `ACCESS_SPIRAM`) is the 11-bit value mask of ESP-IDF's `ext_mem_defs.h`;
  a read-back cannot tell which meaning bit 10 has, and the production
  translation reads 10 bits, enough for 64 MiB of flash.
- `PMU.IMM_HP_CK_POWER_1` reads zero after its tie-high and tie-low bits are
  written: they are write-only pulses, not state.
- After ESP-IDF's `rng_ll_enable` sequence, which pulses the block reset
  after setting it, `TRNG.DATE.CLK_EN` reads zero while the TRNG still
  produces changing output; ESP-IDF's `rng_ll_is_enabled` checks only the
  LP clock and reset, and so does `entropy::source_status`.

## Brownout detector policy

The ESP-IDF bootloader leaves its analog mode-1 brownout reset armed. The
bootstrap replaces it once, right after the SoC initialization and before the
application runs, with ESP-IDF's application policy, as `esp_brownout_init`
does at startup: mode 0 at level 7 (about 2.4 V), with the hardware reset,
flash power-down and RF power-down, and no brownout interrupt (the ESP32-S31
has no `ESP_BROWNOUT_USE_INTR`). A brownout still resets the chip; the
policy sets the threshold and powers the flash and RF down before the reset.

The bootstrap then reads the detector's mode, actions and waits back and
prints `OER_BOOT bootstrap=BROWNOUT`, or stops with
`reason=brownout-policy`, so every boot checks it; the threshold is an
analog register that cannot be read back. Every image boots
through the bootstrap, so the detector is configured before the
application, and before the radio platform takes the analog bus. Rejected:
keeping the bootloader's mode-1 reset, a project-specific level, action or
interrupt, and applying the policy in the radio platform constructor, which
pulled a critical section into PHY code compared with the vendor
library; disabling the detector is never an option.

## Recovery after a lockup

An application can leave LP/PMU state behind that survives a watchdog reset,
the USB-Serial-JTAG (RTS) reset and a reflash: on 2026-09-28 a CPU lockup with
MPLL powered down left the ESP-IDF bootloader looping on
`rst:0x7 HP_SYS_HP_WDT0_RESET` after `Multicore bootloader`. A system reset
through the built-in USB-JTAG (OpenOCD `reset run`, reported by the ROM as
`rst:0x3 SW_SYS_RESET`) cleared it; this board has no EN line wired to the
host. Host tooling that recovers a board applies that escalation; a board that
none of these resets clears needs a person.

Each dedicated IRQ stack is painted once during its first installation, before
IRQ admission. Reinstalling vectors preserves the paint. Thread-mode callers
can sample only their current hart through
`stacks::current_hart_interrupt_stack_free_bytes`; it masks local interrupts
for the bounded SRAM scan and restores their prior enable state. It rejects
sampling from the IRQ stack or before initialization. Measurements describe
observed writes, not the maximum possible depth or unwritten stack reservations.

Each image build leaves its bundle in `target/fw/<image>/`, described by its
`bundle.json` (`tools/image/build`): `bootloader.bin`, `partitions.bin`,
`application.bin`, `otadata.bin`, the packed `runtime.bin`, `runtime.elf`,
`bootstrap.elf` and the check reports. The placement check reads the
`[placement]` contract of the chip's facts (the family layer,
`chips/espressif/platform/family.toml`): among its rules, the interrupt-table
handlers lie in SRAM or ROM, and the panic entry and hook and the
`.critical.*` sections in internal SRAM; `cargo fw` reports a violation, a
HIL image fails on it.

Source or image checks alone do not establish RF behavior on hardware.
