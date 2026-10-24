# How core 1 starts

Core 1 (hart 1) runs no code of this repository unless an image names a root
for it. Three facts hold that together; the first is this note, the second and
third are checked by the image gate (`docs/memory.md`, Stacks).

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
hands core 1 to code nobody started: `esp_hal` clears it in `pre_system_reset`
and holds core 1 in reset from `pre_init` (below), and the early start of both
stages stops a hart other than 0 (`require_boot_hart`, the stage-two entry).

## 2. The esp-hal fork holds core 1 in reset

`esp-hal/src/soc/esp32s31/mod.rs::pre_init` on the primary core calls
`internal_park_core(AppCpu, true)` (stall code `0x86`) and `disable_core1()`
(core and CLIC clocks off, `core1_global_rst_en` set). Only
`cpu_control::start_core1`, reached from `CpuControl::start_app_core*`,
undoes it.

## 3. Nothing in an image reaches the starter

The image gate rejects an image whose call graph reaches the starter symbols
of `[core_start]` in `chip.toml`, and an interrupt-table entry with
`core = 1`, unless that image's `stack.toml` declares core 1 used and proves
its roots.
