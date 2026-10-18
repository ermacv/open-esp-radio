# PHY acceptance scenarios: vendor calibration cross-check

Predecessor HIL catalog, state of 2026-10-16.

These scenarios alternate two firmwares on the same ESP32-S31 board, in
rounds: the pinned vendor firmware is flashed and booted, then the production
image. They compare what each leaves behind:

- the calibration result fields;
- the PHY and modem registers;
- the analog-I2C bytes.

No peer is needed. At the 802.15.4 points the vendor side runs an 802.15.4
reference firmware (channel 15, 21 dBm).

## Gate: the vendor's own boot-to-boot spread

The gate has no fixed ppm, dB or count tolerance; the reference is how much the
vendor firmware itself varies from boot to boot (owner rule, 2026-09-27).

- **Calibration field:** its margin is the widest range that any single
  element spans across the vendor boots. A production element passes when it
  lies inside the vendor range widened by that margin.
- **Register or analog-I2C byte:** the same rule per whole register, with
  that register's own vendor range as the margin.
- **Verdict:**
  - MATCH passes.
  - DIFF fails: something lies outside.
  - INCOMPLETE fails: a field was never reviewed, no register was compared,
    or production did not report a register the vendor could read.
  - A register whose read resets the chip is recorded as unreadable and not
    compared.
- **Read count:** production's analog read count equals that of the published
  register model.

The compared classes:

- **Calibration fields** (about 20, all reviewed):
  - temperature references (common, TX, power, RFPLL), compared signed;
  - TX gain bases (Wi-Fi, BT/802.15.4), the shared cache and the gain
    adjustment, also signed;
  - channel, bandwidth, TX DC rows, RX DC, I2C band, DCODE, RX-gain table
    indices and sensor index, compared unsigned;
  - calibration status, compared on its two completion bits only.

  The current temperature and the tracking-progress counter are excluded.
- **Registers excluded as live state:**
  - PBus read result and command;
  - analog-I2C master control;
  - CFO estimate, AGC RSSI and init words;
  - baseband watchdog status;
  - MAC local time;
  - temperature-sensor control.
- **Registers excluded as clock policy:**
  - modem system and low-power clock enables;
  - PMU active clock power;
  - temperature-sensor clock;
  - HP modem configuration.
- **Excluded at one lifecycle point only:**
  - coexistence low-power clock (both 802.15.4 points);
  - 11b receive-window AGC control and four analog bytes (cold point);
  - one analog byte (restart point; a platform difference not confirmed on
    hardware).

  One analog byte, the live TX capacitor bank, is excluded at every point.

## Cold calibration against the vendor

- Checks: production calibrates cold (its startup artifact is Created) and
  publishes its calibration; this is compared to the vendor cold boot.
- Gate: 10 boots per side; MATCH.
- Hardware: not confirmed as a HIL scenario. A predecessor verification tool
  reported cold MATCH on 2026-09-27: 155 of 155 registers, 35 of 35 fields.

## Calibration after a radio restart

- Checks: the production radio restart reports RF closed and woken; the
  restarted state is compared with the vendor's.
- Gate: 10 boots per side; MATCH.
- Hardware: fail, predecessor HIL runs up to 2026-10-11. 162 of 163 registers
  matched. The differences were the coexistence low-power clock register and
  the temperature references. The earlier verification tool reported restart
  MATCH (156 of 156 registers, 38 of 38 fields) on 2026-09-27.

## Registers with an 802.15.4 session receiving

- Checks: production holds an 802.15.4 session receiving on channel 15. Only
  registers are compared; no calibration fields.
- Gate: 5 boots per side; MATCH.
- Hardware: fail, predecessor HIL runs up to 2026-10-11. One LSB lies outside
  the vendor spread.

## Registers after an 802.15.4 session radio restart

- Checks: the session's radio restart reports RF closed; registers are
  compared.
- Gate: 5 boots per side; MATCH.
- Hardware: not confirmed; no run found.
