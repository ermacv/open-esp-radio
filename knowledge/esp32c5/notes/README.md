# ESP32-C5 notes

Facts about the ESP32-C5 that the predecessor established and that no
register review or pin records elsewhere.

## Vendor pins

- The PHY, Wi-Fi and coexistence archives of
  [`../vendor/artifacts.toml`](../vendor/artifacts.toml) are the submodules of
  the ESP-IDF revision pinned for the ESP32-S31, so both chips compare against
  the same vendor release. The Bluetooth Controller archive is the
  `esp32c5-bt-lib` submodule of that ESP-IDF revision.
- The ROM ELFs of chip revisions v0.x (`rom-rev0`) and v1.0 (`rom-rev100`)
  are pinned separately. The register model applies to v1.0 (`rev100`).
- The pinned ESP-IDF files are the IEEE 802.15.4 driver sources and the
  ESP32-C5 register, SoC and LL headers they include, the counterparts of the
  ESP32-S31 pin.
- The v1.0 ROM carries its own `phy_get_i2c_hostid_` with a different host
  map from the one `libphy.a` links; the library's definition is the one the
  vendor stack runs (register evidence `C5_BLOB_LIBPHY_PHY_I2C`).

## Silicon observations

From predecessor HIL runs on one v1.0 board, 2026-09-27 (register evidence
`registers/esp32c5/evidence/hil-open.toml`):

- Every IEEE 802.15.4 MAC field width the model declares matched the
  silicon, except `CHANNEL` bit 7, which is also implemented; the model keeps
  it unclassified.
- Disabling and re-enabling each vendor modem clock device of the IEEE
  802.15.4 module changed no bit outside the published fields.
- Analog I2C reads of blocks 0x66, 0x6A, 0x6D and 0x61 through the open
  transport equalled the ROM leaf `phy_chip_i2c_readReg_org` byte for byte.

## Errata

- [ESP32-C5 rev 1.0: a USB Serial/JTAG reset boots into UART/SDIO
  download](../../../docs/hardware-errata.md#esp32-c5-rev-10-a-usb-serialjtag-reset-boots-into-uartsdio-download):
  ESP-IDF's modem clock initialisation widens `MODEM_LPCON`
  `CLK_CONF_POWER_ST.CLK_I2C_MST_ST_MAP`, and a USB Serial/JTAG reset then
  enters ROM download mode.
- The stand's handling of a wedged ESP32-C5 USB console is in
  [`host/lab/router`](../../../host/lab/router/README.md).
