# SAR2 state across retained RF wake

The HAL owns TSENS and peripheral SAR-I2C after `take_radio`. Last release
after RF close disables TSENS, its clock, then peripheral power. First acquire
asserts `RSTB_PERIF_I2C` low, completes the named1µs settle, enables
`XPD_PERIF_I2C`, releases reset, then clocks/resets/enables TSENS. It assigns
no literal zero to analog69.04. Initial esp-hal setup precedes the handoff.

The existing restore enum carries resource ownership: idle, poisoned, or acquired
with an empty/pending calibration restore. Pending restore prevents release;
unavailable resources prevent calibration captures. An incomplete reset settle
poisons the owner and prevents neutral-root return. No reference count is needed.

SOURCE: ESP-IDF `4d59230ddff16327812782151ef0afef202dc6d7`, IEEE application
SHA256 `b033e780ca40ae732067504436951c7726d68bcb1bdd4b34180c037740deb394`,
linked ELF SHA256 `3ace8cc6eddeae7ce6729b1522a27cd53336b4d168abfef1795c11c7ad3462b7`.
The link map selects `esp_phy/phy_override.c.obj::phy_set_tsens_power`
(`0x2f0030cc`, size `0x26`), which calls the IDF temperature power owner.
The archive/ROM MMIO-only leaf is not this linked function.

SOURCE: `esp_phy/src/phy_init.c::esp_phy_disable` closes RF and calls
`phy_xpd_tsens`; the linked override releases temperature power.
`esp_hw_support/sar_tsens_ctrl.c::temperature_sensor_power_release` disables
TSENS, its bus clock and its SAR-I2C reference on last release.
`esp_hw_support/sar_periph_ctrl_common.c::regi2c_saradc_disable` clears
`PMU.XPD_PERIF_I2C` (linked `0x2f009f40..0x2f009f4c`). On first acquire,
S31 `regi2c_ctrl_ll.h::regi2c_ctrl_ll_i2c_sar_periph_enable` asserts reset,
waits1µs, powers the bus and releases reset (linked `0x2f009ffa..0x2f00a026`).
TSENS clock/reset/enable and range follow in the vendor path. There is no
analog69.04=0 assignment in this identified vendor close/wake chain.

SOURCE: cold `libphy.a[phy_init.o]::phy_rf_init` calls
`phy_i2c_sar2_init_code(1400)` when the initialization field is zero. Complete
ROM `0x2f82a444` writes the high byte,5, to the field and the low byte to69.03.
Retained `phy_wakeup_init` does not repeat that cold initialization.

Measured source `4d679728` on `47d38933`, diagnostic app
`b21bed3e0400a674c314e14ee3fb1bfb2c86985c5a5e1df68db78ddad24ce2f5`:
IEEE restart5 `1792535209115-00090c8d` and Wi-Fi restart10
`1792535406587-0009653a` retain `0x65`/init5 before first close, then read
`0x60`/init0 after wake and before final close. Every final pair equals vendor.
Existing same-point vendor references give IEEE N10 MATCH193/193 MMIO,39/39 analog
and Wi-Fi N20 MATCH19/19 groups,163/163 MMIO,39/39 analog with69.04 compared
at both restart points. The original raw N5/N10 DIFF summaries remain archived;
the capture-time reviews reproduce them exactly before reevaluation.

Wi-Fi cold control `1792535709069-00098258` retains the existing0x65/0x60 point
difference: production is cold registration, vendor follows its Wi-Fi-start RF
wake. Its separate cold lifecycle exclusion is unchanged; N11 cold references
give MATCH162/162,35/35. Restart compares69.04 normally: the observed0x60/0x60
after the HAL resource-lifecycle correction disproves the PSRAM/MPLL exclusion.
IEEE cold control `1792536001039-0009f671` has0x65/0x65 at69.04 and retained
I2C TX rate; N41 same-point references give MATCH193/193,39/39. Both cold
controls retain their raw N1 DIFF summaries; no calibration/rule change is used.

The diagnostic reads `TEMPERATURE_SENSOR_SAR2_STATUS` after registration,
before close and after wake under the existing PHY clock reference. It adds no
power reference and never reads with clocks off. Vendor attribution uses the
linked SOURCE and final captures, without an intermediate vendor analog sample.
