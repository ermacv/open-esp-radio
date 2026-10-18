# ESP32-S31 boot-state probe

A standalone image that reads, on the board, boot state the `ermacv/esp-hal`
fork relies on but neither ESP-IDF nor the PAC settles:

- **TRNG.** Its LP clock, configuration and interrupt words after
  `esp_hal::init`, and four output words.
- **PMA.** Every `pmacfg`/`pmaaddr` pair as the ESP-IDF bootloader left it,
  then after `esp_hal::init`, which programs PMA entry 7 for the 64 MiB
  external-memory aperture. Each line decodes the NAPOT region.
- **Flash MMU page number.** An unused, invalid MMU entry is written with
  10-, 11- and 15-bit page numbers and read back, then restored. The PAC
  publishes a 10-bit `PADDR`; ESP-IDF's `ext_mem_defs.h` names an 11-bit value
  mask and 32768 pages.
- **`PMU.IMM_HP_CK_POWER_1`.** Whether the CPLL tie-high bits read back set
  after a write that keeps the running CPLL powered, or are pulses.
- **Watchdog disable.** TIMG0's watchdog is armed with a 1 s system-reset
  stage, disabled through esp-hal and left for 3 s. A second `PROBE-BOOT`
  before `PROBE-DONE` means the disable did not apply.

The image boots through the ESP-IDF bootloader that `espflash` bundles, the
one production images use; `cargo fw flash` writes image bundles and catalog
images only, so `espflash` flashes this ELF under a board lease. From the
repository root, with the board's port from `cargo stand board check esp32s31`:

```console
(cd verification/esp32s31/hardware/boot-state-probe && cargo build --release)
elf=verification/esp32s31/hardware/boot-state-probe/target/riscv32imafc-unknown-none-elf/release/oer-esp32s31-boot-state-probe
cargo stand lease --board esp32s31 --air none --flashed esp32s31-boot-state-probe \
  --device esp32s31 --application "$elf" -- sh -c \
  "espflash flash --chip esp32s31 --port PORT $elf && timeout 25 espflash monitor --chip esp32s31 --port PORT --non-interactive --elf $elf"
```
