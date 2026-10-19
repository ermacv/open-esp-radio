# Wi-Fi bring-up order against common power

**Fact.** On the ESP32-S31, after a full common-power cycle (the last radio
client left common power, the cold baseline was restored and RF was closed),
Wi-Fi works again only if it prepares the shared PHY (RF wake) and enables
coexistence **before** it enters common power again. The first start works in
either order.

**Order that works:** prepare the shared PHY, enable coexistence, then enter
common power. Bluetooth LE and IEEE 802.15.4 entering power before the PHY
passed their scenarios, but none of their scenarios covered a full power
cycle with an RF wake.

Source: stand measurement, 2026-10-11/12 (record outside this repository).

## Conditions

- Scenario: station stopped, radio restarted (full common-power cycle with an
  RF close), station rejoins, on the HIL stand. Board
  revision, supply and RF environment not recorded.
- Order changes were applied on the restart and, where stated, on the first
  start.

## Experiments

Runs of 2026-10-11/12 (exact date per run not recorded here):

| First start | Restart | Result |
| --- | --- | --- |
| PHY, coex, power | PHY, coex, power | passed 3/3 |
| power, PHY, coex | power, PHY, coex | failed 3/3 |
| PHY, coex, power, over the new HAL/PHY/radio layers | same | passed (1 rep) |
| PHY, coex, power, with the new Wi-Fi code | same | passed (1 rep) |
| PHY, coex, power | power, PHY, coex | failed (1 rep) |
| power first | RF wake before power, coex after | failed (1 rep) |
| power first | coex before power, RF wake after | failed (1 rep) |

Bounded search on the restart in the power-first order, 2026-10-12, one
repetition each, all failed:

1. The repeated power entry skips the baseband-only reset pulse.
2. After RF wake and coexistence enable, the power entry's modem clock writes
   are applied again (HP-active and sleep ICG selection, modem register bus
   clock, Wi-Fi power clock map, shared modem clock map, modem source clocks).
3. As 2, plus the PHY calibration clocks and the 160 MHz PHY I2C source.

## Failure shape

- Station stop and radio restart are accepted; then nothing is logged.
- Core 0 sits in the station's rejoin, in its receive service; the logger is
  starved.
- The Wi-Fi deadline watchdog (TIMG1) resets the chip. 10 s and 60 s budgets
  only move the reset later.

## What it rules out, what stays open

Ruled out as the cause: the new HAL/PHY/radio layers; the order of the Wi-Fi
module clocks against the RF wake; moving only one of RF wake and coexistence
before power; each register write of the power entry, re-applied or skipped.

Open (not measured): the platform references the power entry takes (MPLL,
160 MHz source, analog-I2C master), which the RF wake in the working order
runs without; the cold-baseline capture at the power entry. ESP-IDF's
`esp_wifi_stop` keeps the modem power domain on, so it never takes this path.

The earlier hypothesis that the PHY-first order is a latent defect (the power
entry's baseband reset after PHY registration) is contradicted by these runs.
