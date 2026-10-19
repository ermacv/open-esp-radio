# ESP32-S31 staged application boot

This platform composes ESP-HAL for ESP32-S31-Function-CoreBoard-1. It has no
test protocol, radio role, executor or network-stack dependency.

| Component | Responsibility |
| --- | --- |
| `board` | Board-specific 16-MiB PSRAM at 250 MHz and 16-MiB Flash configuration; adoption of the live mapping |
| `bootstrap` | Flash entry, PSRAM initialization, image validation/CRC, relocation, Flash tuning and non-returning handoff |
| `runtime` | Stage-two entry, SRAM section initialization, mapping adoption, vector handoff, per-core interrupt stacks and the image's one panic entry |
| `layout` | Address map, the stage-two placement and the stage-two header/checksum shared with the host packer and auditor |
| `linker` (`chips/espressif/staged-layout/linker`, `chips/esp32s31/layout/linker`) | Semantic code, data, DMA and stack sections over the `layout` symbols |
| `chips/esp32s31/platform/partitions` | Application partition layout |
| `chips/esp32s31/platform/stack.toml` | Stack policy: each stack's root function, storage and reserve, the move limit and the frame-coverage review (`stack-coverage.toml`) |

The runtime owns every image's `#[panic_handler]`. It writes a bounded record
of the panic to `.rtc_fast.persistent` without formatting (the end of the
location's file path, line, column, the start of the message when it is a
static string, hart and, on an interrupt stack, the interrupted PC), so the
panic path stays a short leaf in every stack bound. An image with
`panic-hook` names its hook with `panic_hook!`, which the entry calls after
the record to record more of its own state the same way; it returns. An
image's own reset-retained records are `retained_static!` cells, one holder
at a time without waiting, with their claim in the SRAM critical BSS. Every
image then resets, and its next boot reads and clears the record with
`panic::take_previous` and prints it.
The hook receives `panic::PanicDetails`, rather than the original `PanicInfo`.
Interrupt-context panics retain hart and interrupted PC, but omit location and
message: compiler metadata can live in cached PSRAM. Task-context panics keep
their metadata because their code and stack already require the live mapping.
The hook and all state it reads must stay in SRAM or retained memory and take
no locks.

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
`Handoff`. The application calls `esp_hal::init`, then
`handoff.adopt(INTERRUPT_TABLE, BootstrapPsram(peripherals.PSRAM))`: the
board's `BootstrapPsram` implements the staged layout's
`AdoptsBootstrapMapping`, the promise that it adopts the bootstrap's mapping
without resetting or remapping PSRAM. The board does not link the runtime,
whose panic entry would otherwise reach the bootstrap through the board.
`adopt` also installs the interrupt table, which silences every source it
lists, reinitializes vectoring and installs the per-core SRAM interrupt
stack. It returns `Adopted`, whose `enable_interrupts` sets global interrupt
enable once the application has bound its timers and executor handlers: a
source is routed only by its token's owner, after it bound its handler.
The runtime is the only owner of each hart's active vector table (MTVT):
every CPU interrupt enters through its stack-switching vector entry and
esp-hal's shared dispatcher. The esp-hal fork only vectors the
inter-processor call's line (CLINT software interrupt 3) and runs the posted
function from that dispatcher, instead of writing its own handler into the
slot; `enable_interrupts` panics when any hardware-vector slot
differs from the runtime's entries, so a later direct binding cannot run on
the interrupted stack.

Each image declares its peripheral interrupt sources once with
`oer_espressif_staged_runtime::interrupt_table!` (source, handler, level,
core; see [`oer-interrupt-table`](../../../portable/interrupt-table/README.md)):
the macro links each handler into its source's slot of esp-hal's
`__EXTERNAL_INTERRUPTS` and creates one token per source; the image hands its
`INTERRUPT_TABLE` to `Handoff::adopt`. Each 12-byte entry holds the `Interrupt` (`u16`), the
`Priority` (`u8`), the `Cpu` (a 4-byte C enum at offset 4) and the handler
address at offset 8 (zero for an entry a `cfg` leaves out); compile-time
assertions in `interrupts.rs` keep that layout. Installing a hart's
interrupt stack silences every source of that hart's entries and checks the
slots; `enable_interrupts` checks again that every slot holds its
handler and every table source is silent or routed to its level. An owner
routes its source with `oer_espressif_esp_hal_soc::interrupt_table::enable` and
its token, on the table's core only, and silences it with `disable`; the
matrix lives in that adapter so that owners in `chips/` reach it. esp-hal's PMP region setup
(`ESP_HAL_CONFIG_ENABLE_PMP`) is off in the root Cargo configuration, because
its single no-execute data region would span stage two's PSRAM code.

Every image uses one placement: PSRAM for code, ordinary data and a 192-KiB
CPU0 task stack; SRAM for interrupt entries, hot code, critical state, DMA
storage and two 32-KiB interrupt stacks. No image runs a task on CPU1, and
the runtime has no CPU1 PSRAM task stack. These board sizes are not universal
chip capabilities.

Because code runs from PSRAM, the adopted mapping keeps the PSRAM function
clock, and through it MPLL, referenced in ESP-HAL's clock tree for the life of
the image. A driver that releases MPLL therefore never powers it down under
PSRAM.

## Boot state on the board

Reads on the board show what the bootloader leaves and what esp-hal relies on:

- The `espflash`-bundled ESP-IDF bootloader already programs PMA entry 7 as
  the locked 64 MiB NAPOT external-memory aperture at `0x5000_0000`, the
  value esp-hal's `pre_init` writes; entry 15 covers the flash window
  `0x4000_0000` (512 MiB), entries 12-14 the internal memories, and entry 8 is
  unused. Newer ESP-IDF bootloaders that put the flash window top in entry 7
  would conflict with esp-hal's locked write.
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
- Disabling the TIMG watchdog through esp-hal holds: no reset arrives past
  the armed timeout.

## Brownout detector policy

The ESP-IDF bootloader leaves its analog mode-1 brownout reset armed. The
bootstrap replaces it once, right after `esp_hal::init` and before the
application runs, with ESP-IDF's application policy, as `esp_brownout_init`
does at startup: mode 0 at level 7 (about 2.4 V), with the hardware reset,
flash power-down and RF power-down, and no brownout interrupt (the ESP32-S31
has no `ESP_BROWNOUT_USE_INTR`). A brownout still resets the chip; the
policy sets the threshold and powers the flash and RF down before the reset.

The bootstrap then reads the detector's mode, actions and waits back and
prints `OER_BOOT bootstrap=BROWNOUT`, or stops with
`reason=brownout-policy`, so every boot checks it; the threshold is an
analog register esp-hal writes but cannot read back. Every image boots
through the bootstrap, so the detector is configured before the
application, and before the radio platform takes the analog bus. Rejected:
keeping the bootloader's mode-1 reset, a project-specific level, action or
interrupt, and applying the policy in the radio platform constructor, which
pulled esp-hal's critical section into PHY code compared with the vendor
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
`[placement]` contract of `chip.toml`: among its rules, the interrupt-table
handlers lie in SRAM or ROM, and the panic entry and hook and the
`.critical.*` sections in internal SRAM; `cargo fw` reports a violation, a
HIL image fails on it.

Source or image checks alone do not establish RF behavior on hardware.
