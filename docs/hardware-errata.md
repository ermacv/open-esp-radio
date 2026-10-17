# Hardware errata

Hardware and ROM behaviour that the stand's boards show and that the chip
documentation does not describe. The ESP32-C5 entries concern the stand's
peer boards, which run ESP-IDF firmware for tests against the ESP32-S31. Each entry names the affected chip and
revision, how the fault shows, the exact condition that triggers it, how it
was established and what the repository does about it. A workaround stays
with its owner; this page is the canonical description the owners link to.

## ESP32-C5 rev 1.0: a USB Serial/JTAG reset boots into UART/SDIO download

**Chip:** ESP32-C5 revision v1.0 (ROM `esp32c5-eco2-20250121`), observed on
the stand's `esp32c5` board.

**Symptom.** A reset requested over the chip's USB Serial/JTAG port (the
host pulses the virtual RTS line, as `espflash`, `esptool` and the stand's
flash commands do) restarts the chip into ROM download mode through UART0 or
SDIO instead of booting from flash:

```text
rst:0x15 (USB_UART_HPSYS),boot:0x0 (DOWNLOAD(UART0/SDIO_FEI_FEO))
wait uart0/sdio download
```

In that mode the ROM does not serve USB Serial/JTAG: CDC control requests
fail with a STALL (`EPIPE` on the host), `espflash` and OpenOCD cannot connect,
and a USB port reset drops the device from the bus (`error -71`). Only the
board's RST button, which resets the whole chip, recovers it. A normal boot
reads `boot:0x18` or `boot:0x58` (SPI fast flash boot), and a download
request over USB reads `boot:0x8`.

**Trigger.** The field `clk_i2c_mst_st_map` of `MODEM_LPCON`
`CLK_CONF_POWER_ST` selects the PMU power states in which the clock of the
analog register I2C master runs, one bit per state: SLEEP, MODEM, ACTIVE.
The ROM and applications that leave the field alone have ACTIVE only. ESP-IDF's
modem clock initialisation (`components/esp_hw_support/modem/port/esp32c5/modem_clock_impl.c`,
the `MODEM_CLOCK_DOMAIN_I2C_MASTER` default) sets ACTIVE and MODEM in every
application, whether or not it uses the radio. The register belongs to the LP
domain and keeps its value through the HP-system reset that a USB
Serial/JTAG RTS pulse performs; with the MODEM bit still set, the ROM of this
revision takes the UART/SDIO download path. Why the ROM does so is not
documented; the condition is established by experiment. The neighbouring
`clk_lp_apb_st_map`, which ESP-IDF also widens to ACTIVE and MODEM, has no
effect.

This is a different fault from the ESP-IDF USB Serial/JTAG console fix for the
same revision that keeps UART0's clock enabled (`IDFGH-17050`); ESP-IDF
master `4d59230d` and release v6.1 both contain that fix and both show this
fault.

**How it was established.**

- Every ESP-IDF image hung on a USB reset (the IEEE 802.15.4 and OpenThread
  peers on ESP-IDF master `4d59230d` and on v6.1, with the radio enabled or
  disabled), while esp-hal images booted normally after the same reset.
- The LP-domain registers of a running esp-hal image and a running ESP-IDF
  image, read over the chip's JTAG with OpenOCD, differ in PMU state
  configuration, `LP_ANA` brown-out control, `LPPERI` clock enables and
  `MODEM_LPCON` `CLK_CONF_POWER_ST`.
- Writing the esp-hal values into the running ESP-IDF image before the USB
  reset, group by group, isolated the fault: restoring `clk_i2c_mst_st_map`
  alone to ACTIVE made the ESP-IDF image boot normally after the reset;
  restoring every other differing register left it hanging.
- A reset through the chip's JTAG (OpenOCD `reset run`, cause
  `JTAG CPU reset`) is not affected.

**Workaround.**

- An application that never enters the PMU MODEM state (no power management,
  no light sleep with the modem on) restores ACTIVE only in
  `clk_i2c_mst_st_map` at start and after each radio enable. ESP-IDF peer
  firmware for the stand must do so; with it, USB resets and flashing over a
  running peer work.
- Firmware for the ESP32-C5 must not add the MODEM state to that map unless it
  also avoids USB Serial/JTAG resets.
- Any other ESP-IDF image on an ESP32-C5 rev 1.0 without the workaround is
  reset or flashed through JTAG, not over USB Serial/JTAG.

**Limits.** Observed on one board of revision v1.0. Other revisions and
boards are unverified.
