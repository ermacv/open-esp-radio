# How core 1 starts

Every current image keeps the secondary HP hart contained. The public HAL
cannot start or release a secondary hart. The staged build/run check audits
linked critical-section entries against the image's declared hart roots.
The platform HP-hart list defines the geometry and the boot role.

## 1. The mask ROM starts core 1 only at an explicit boot address

SOURCE: `rom` of `vendor/artifacts.toml` (`esp32s31_rev0_rom.elf`, SHA-256
`a52ad751…93d5ab87`), disassembled, not committed.

- Reset entry `_init` (`0x2f801710`): `csrr t0, mhartid; bnez t0, do_core1_wake`.
  `do_core1_wake` and the cold path both reach `normal_boot` (`0x2f801790`),
  which sets up the ROM's own trap vector and ROM stack and, for hart ≠ 0,
  reads the word at `0x2070_0164` (`ets_set_appcpu_boot_addr` writes it,
  `system_hal_get_appcpu_boot_addr` reads it). If its bit 31 is set it jumps
  to the address with that bit cleared.
- `main` (`0x2f804652`): for hart 1 it calls `system_hal_get_appcpu_boot_addr`
  in a loop until the word is non-zero, then calls it
  (`riscv_pma_flash_boot_cfg` first). It initializes no UART, flash or
  interrupt routing for hart 1.
- So a released core 1 waits in ROM, on the ROM's stack, with no interrupt
  source routed to it, and leaves ROM only through a non-zero boot address.

SOURCE: ESP-IDF `4d59230ddff16327812782151ef0afef202dc6d7`,
`components/esp_system/port/cpu_start.c`: `call_start_cpu0` clears the address
(`ets_set_appcpu_boot_addr(0)`) first thing; `start_other_core` is the one
place that starts core 1 (`esp_cpu_unstall(1)`, clock and reset release,
`ets_set_appcpu_boot_addr(call_start_cpu1)`); single-core mode instead gates
core 1's clocks and holds `GLOBAL_RST_EN`.

A stale non-zero boot address left by an earlier image is the one way ROM
hands core 1 to code nobody started: the bootstrap clears it and holds core 1
in reset (below), and the entries of both stages (`_start` of the bootstrap,
`_runtime_start` of stage two) stop a hart other than 0 before any code runs.

## 2. The bootstrap holds core 1 in reset

The HAL's SoC initialization (`oer_esp32s31_hal::boot::soc`, `contain_secondaries`),
which the bootstrap runs first, iterates the generated secondary identities,
requests their stalls, gates their core and CLIC clocks, holds their global
resets and clears their indexed ROM boot addresses
(SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/cpu_control.rs:6-96`,
ESP-IDF `system_internal.c:61-100`). The public HAL provides no operation that
releases this containment or publishes a secondary entry.

## 3. Public API and linked-image checks

Calling secondary start or clock/reset release through the public HAL fails
at compile time. The image gate additionally rejects linked function symbols
matching `[core-start]` in `chip.toml` when the image declares a secondary task
stack unused; this symbol inventory does not identify inlined launch code.
Interrupt-table routes and storage must also agree with the image's declared
per-hart stacks. The critical-section check follows the existing call
resolutions from the image's declared task roots, every interrupt-table handler
and exception root, rejecting linked section entry symbols and their inline
copies on a foreign hart.

The register model describes stall-code and status lanes, clock/reset words,
reset causes, secondary ROM boot words and matrix banks as indexed families.
The local CLIC address is context-relative, not an absolute hart-indexed bank;
its owned activation remains the boot-hart controller.

SOURCE: `hw1-20261026-boot-07`, `research/hw-interrupt-facts.md`: inherited
MNLBITS after ROM is one. This is distinct from the header's reset value;
HAL writes its selected MNLBITS explicitly. Boot-word readback while the
secondary is held establishes storage semantics, not secondary execution.
