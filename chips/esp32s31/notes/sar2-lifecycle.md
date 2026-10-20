# SAR2 state across retained RF wake

The diagnostic image reads the published `TEMPERATURE_SENSOR_SAR2_STATUS`
field after registration, before RF close and after retained wake, while the
existing PHY clock reference is held. It does not read with clocks off,
acquire an extra power reference or write an analog field.

IEEE restart observation `1792532634016-00020f57` (clean `35ceab5d`) reads
`0x65` / initialization field 5 at all four checkpoints, including final close.
The restarted final capture is also `0x65`; the pinned vendor reads `0x60`.
The unchanged comparator reproduces raw N1 DIFF, then matches 193/193 MMIO
and 38/39 analog with six existing/current vendor restart captures. Only
69.04 remains different, vendor `[96,96]`, production `[101,101]`.

SOURCE: ESP-IDF `4d59230ddff16327812782151ef0afef202dc6d7`, IEEE application
SHA256 `b033e780ca40ae732067504436951c7726d68bcb1bdd4b34180c037740deb394`,
linked ELF SHA256 `3ace8cc6eddeae7ce6729b1522a27cd53336b4d168abfef1795c11c7ad3462b7`.
The link map selects `esp_phy/phy_override.c.obj::phy_set_tsens_power`
(`0x2f0030cc`, size `0x26`), which calls the IDF temperature power owner.
The archive/ROM leaf that only changes TSENS power is not this linked function.

Wi-Fi restart control `1792533179530-000408cc` (clean `388ab678`) has the
same four `0x65` / 5 checkpoints and final vendor96/production101. Reusing ten
existing vendor restart captures yields MATCH163/163 MMIO,38/38 analog; its
existing reviewed Wi-Fi69.04 exclusion remains unchanged. That MATCH does not
mean the SAR2 field matches. Both diagnostic runs have the identical application
SHA256 `8395332f307327d255ecb78f05658352b9f9e0f1295e2235fc14595de5b080a6`.

SOURCE: `esp_phy/src/phy_init.c::esp_phy_disable` closes RF and calls
`phy_xpd_tsens`; the linked override releases temperature power.
`esp_hw_support/sar_tsens_ctrl.c::temperature_sensor_power_release` disables
TSENS, its bus clock and its SAR-I2C reference on last release.
`esp_hw_support/sar_periph_ctrl_common.c::regi2c_saradc_disable` clears
`PMU.XPD_PERIF_I2C` on last release (linked `0x2f009f40..0x2f009f4c`).
On first acquire, `esp32s31/hal/regi2c_ctrl_ll.h::regi2c_ctrl_ll_i2c_sar_periph_enable`
asserts `RSTB_PERIF_I2C` low, waits 1 µs, enables `XPD_PERIF_I2C`, then releases
reset (linked `0x2f009ffa..0x2f00a026`). TSENS clock/reset/enable and range follow.
There is no direct analog69.04=0 assignment in this identified close/wake chain.

SOURCE: cold `libphy.a[phy_init.o]::phy_rf_init` calls
`phy_i2c_sar2_init_code(1400)` when this field is zero. Complete ROM
`0x2f82a444` writes the high byte, 5, to the field and the low byte to69.03.
Retained `phy_wakeup_init` does not call this cold initialization.

OER's temperature power leaves change only TSENS power. Its general analog-I2C
reset is conditional on the peripheral already being powered off; temperature
close does not release that power. The vendor power/reset lifecycle is therefore
absent. The observed retained-field difference is real. Attribution of the
vendor's zero to that reset follows from SOURCE and final captures; the vendor
register was not sampled between the individual shutdown/wake operations.
The observation does not establish a calibration error or change comparison rules.
