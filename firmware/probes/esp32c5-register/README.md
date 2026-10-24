# ESP32-C5 register probe

A standalone image that checks the published
[ESP32-C5 register model](../../../chips/esp32c5/registers/README.md) on the
board. It opens the IEEE 802.15.4 MAC clocks as esp-radio does, writes all
ones to selected read-write MAC registers through a raw `oer-mmio` block over
the MAC's declared words, claimed after the typed owner is dropped, and
reads each word back. The bits that stay set are the implemented ones, so
every `PROBE` line is `MATCH` when the silicon has the field widths the model
declares and `DIFF` otherwise. The MAC stays idle; no command is issued.

Before that, the image drives the modem clock owner of the production PAC
(`oer_esp32c5_pac::ModemClockRegisters`). For each vendor modem clock device
of the IEEE 802.15.4 module it disables and re-enables the device and checks
the vendor enable check both ways. It then pulses the IEEE 802.15.4 MAC reset
and checks that both lines read released. Finally it reads the four modem
clock words through esp-pacs and checks that no bit outside the published
fields changed.

Last, it reads registers 0 to 15 of the analog blocks 0x66 (BBPLL), 0x6A
(BIAS), 0x6D (DIG_REG) and 0x61 (ULP_CAL) through the production analog I2C
owner (`oer_esp32c5_pac::PhyI2cRegisters`) and compares each byte with the
ROM leaf `phy_chip_i2c_readReg_org`, called with the same block, read mask
and host. A block whose read does not complete is `INCOMPLETE`.

| Register | Expected implemented bits |
| --- | --- |
| `CHANNEL` | frequency code 6:0 and the opaque bit 7 |
| `TX_POWER` | power code 4:0 |
| `EVENT_ENABLE` | thirteen events 12:0 |
| `COEX_PTI` | TX/RX PTI 3:0, ACK PTI 7:4, bit 8 |

`cargo fw flash` writes image bundles and catalog images only, so `espflash`
flashes this ELF under a board lease. From the repository root, with the
board's port from `cargo stand board check esp32c5`:

```console
cargo fw probe oer-firmware-probes-esp32c5-register
elf=target/fw-probe/riscv32imac-unknown-none-elf/firmware/oer-firmware-probes-esp32c5-register
cargo stand lease --board esp32c5 --air none --flashed esp32c5-register-probe \
  --device esp32c5 --application "$elf" -- sh -c \
  "espflash flash --chip esp32c5 --port PORT $elf && timeout 30 espflash monitor --chip esp32c5 --port PORT --non-interactive --elf $elf"
```

The image boots through the ESP-IDF second-stage bootloader that `espflash`
writes and uses esp-hal's standard linker script.

On the stand's ESP32-C5 v1.0 `CHANNEL` reads back `0xff` after an all-ones
write. The model records bit 7 as `BIT_7_OPAQUE`, with its meaning and reset
value unknown, alongside the public seven-bit frequency code. SOURCE:
`HIL_OPEN_C5_REGISTER_PROBE_2026_09_27` and
`HIL_OPEN_C5_CHANNEL_JTAG_2026_10_23`. The other measured widths match the
published fields. Width experiments write registers; the JTAG repeat uses
a halted target with OpenOCD's WDT-disable hook suppressed.
