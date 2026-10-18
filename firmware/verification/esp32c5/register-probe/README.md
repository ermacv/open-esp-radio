# ESP32-C5 register probe

A standalone image that checks the published
[ESP32-C5 register model](../../../../registers/esp32c5/README.md) on the
board. It opens the IEEE 802.15.4 MAC clocks as esp-radio does, writes all
ones to selected read-write MAC registers through the generated raw PAC and
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
| `CHANNEL` | frequency code 6:0 and the unclassified bit 7 |
| `TX_POWER` | power code 4:0 |
| `EVENT_ENABLE` | thirteen events 12:0 |
| `COEX_PTI` | TX/RX PTI 3:0, ACK PTI 7:4, bit 8 |

`cargo fw flash` writes image bundles and catalog images only, so `espflash`
flashes this ELF under a board lease. From the repository root, with the
board's port from `cargo stand board check esp32c5`:

```console
(cd verification/esp32c5/hardware/register-probe && cargo build --release)
elf=verification/esp32c5/hardware/register-probe/target/riscv32imac-unknown-none-elf/release/oer-esp32c5-register-probe
cargo stand lease --board esp32c5 --air none --flashed esp32c5-register-probe \
  --device esp32c5 --application "$elf" -- sh -c \
  "espflash flash --chip esp32c5 --port PORT $elf && timeout 30 espflash monitor --chip esp32c5 --port PORT --non-interactive --elf $elf"
```

The image boots through the ESP-IDF second-stage bootloader that `espflash`
writes and uses esp-hal's standard linker script.

On the stand's ESP32-C5 v1.0 every width matches the model except `CHANNEL`
bit 7, which is also implemented; the register model records that bit as
unclassified with this observation as its evidence.
