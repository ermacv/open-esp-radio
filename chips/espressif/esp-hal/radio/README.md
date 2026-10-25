# oer-espressif-esp-hal-radio

The handoff of the ESP32-S31 radio blocks from esp-hal to the radio HAL
(docs/architecture.md, "Hardware"), and the radio's platform
clocks on esp-hal.

- `take_radio(RadioSingletons, &Psram, PllPowerReleased)` consumes esp-hal's singletons of the
  blocks the radio owns whole (`I2C_ANA_MST`, `MODEM_SYSCON`, `MODEM_LPCON`,
  `WIFI`, `BT`, `IEEE802154`, `LP_TSENS`) and returns the HAL's
  `RadioHardware` and the clock provider. The adopted `Psram` proves that
  esp-hal holds its never-released MPLL reference, so the radio's MPLL
  requests never cross zero. `PllPowerReleased` comes from the bootstrap root's `BootstrapPllPower::release()`,
  so the bootstrap's owner and the radio root never exist at once. Its one `unsafe` asserts the radio handoff
  and the analog-bus ownership, which the consumed singletons make true.
- Blocks where the radio owns only single words (PMU, HP_SYS_CLKRST,
  LP_AON_CLK_RST, LP_PERICLKRST, the interrupt matrix) stay esp-hal's; their
  shared words are reviewed in `chips/esp32s31/registers/shared-words.toml`.
- `PlatformClocks<EspHal>` implements the HAL's `PlatformClockProvider`: the
  `MODEM_LPCON.CLK_CONF` gates through `clock::ll`'s counted acquire and
  release, MPLL and the 160 MHz PLL output through the clock tree's request
  and release. A reference goes back only through the guard its request
  returned.

The guarantee has a known limit: esp-hal's `X::regs()` is a safe function
that needs no singleton, so consuming a singleton excludes drivers that take
it by value, not a direct `regs()` call. The repository's `clippy.toml`
disallows `regs()` of the consumed peripherals with registers (`WIFI` and `BT`
are virtual).

The esp-hal half needs the `esp32s31` feature and the chip target; host tests
run the provider over a recording backend.
