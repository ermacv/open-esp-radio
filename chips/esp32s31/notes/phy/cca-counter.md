# PHY CCA counter

`PHY_BB_RECOVERED_GAPS.CCA_COUNTER_RESULT_0` and
`CCA_COUNTER_RESULT_1` are hardware measurement results over a window,
not calibration settings. The vendor leaves these results unconsumed.
The vendor comparison checks that both words are readable in every
production capture and excludes their values. Missing or unreadable
production samples yield INCOMPLETE. `CCA_COUNTER_CONTROL`, including the
window and control bits, remains compared by value.

## Register semantics

SOURCE: complete ESP32-S31 rev0 ROM ELF, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`;
`phy_set_cca_cnt` at `0x2f8256cc`, size `0x2e`, and
`phy_get_cca_cnt` at `0x2f8256fa`, size `0x24` ([#240](https://github.com/ermacv/open-esp-radio/pull/240)).

| Register | Address | Fields and ROM use |
| --- | --- | --- |
| `CCA_COUNTER_CONTROL` | `0x20107c58` | `WINDOW` (26:0) is replaced from arg0, preserving bits 31:27. Nonzero arg1 additionally sets `CONTROL` (28:27) in a fresh read-modify-write. |
| `CCA_COUNTER_RESULT_0` | `0x20107c5c` | `RESULT` (26:0) is copied to out[0]; `FLAG` (27) is returned as 0/1. |
| `CCA_COUNTER_RESULT_1` | `0x20107c60` | `RESULT` (26:0) is copied to out[1]. |

The control bits' individual roles, the flag's physical meaning and the
window's physical units are not established by these ROM leaves.

SOURCE: s31-b vendor and diagnostic captures in
[#254](https://github.com/ermacv/open-esp-radio/pull/254), E1/E3.
The vendor result is 4 across five reads without growth. Explicit diagnostic
windows of `0x100000` produce `0x3c6ce` and `0x25130` on two boots;
`0x10000` produces zero. The result depends on the measurement window
rather than a retained calibration value.

## Window trigger and receive sequence

SOURCE: s31-b hardware watchpoints and call bisection in #254, E4/E5;
HIL runs `1792554053095-001588f7` and `1792570921393-0017a8df`.
The pinned IDF revision is `4d59230ddff16327812782151ef0afef202dc6d7`,
vendor application SHA-256
`e7c115dedfdb1fbf65372ef18655855b2f7d5b415f00de5a2d8347fc0940272a`,
vendor ELF SHA-256
`7e609cb94b84527a5ed4f12adee77a0321ef297b4799b4da5f590ffb75eb6078`;
the complete ROM ELF is identified above.

Releasing `MODEM_SYSCON_RADIO.MODEM_RST_CONF.RST_WIFIBB` restarts the
window. On the vendor PM wake path reached from
`esp_wifi_set_promiscuous(true)`, `pm_wake_up` calls
`pm_disconnected_wake`, then `wifi_rf_phy_enable`, the Wi-Fi clock wrapper
and `modem_clock_module_enable`. Its leaf
`modem_clock_wifi_bb_configure(true)` asserts and releases `RST_WIFIBB`
before enabling baseband clocks. The reset word is `0x100`, then zero,
at `0x20109c10`. DWARF identifies `modem_syscon_ll_reset_wifibb` in
`components/hal/esp32s31/include/hal/modem_syscon_ll.h:283–284`, inlined
into `components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c:70`.

Immediately after reset release, two probes read `+0x5c=0x0000000b`,
followed by `0x08010000`; `+0x58` stays `0x80010000`. No processor store
to `+0x58` or autonomous result read occurs during the observed vendor
boot with verified watchpoints on both harts. In an uninterrupted capture,
`+0x60` changes from zero after `esp_wifi_start()` to 4 after
`esp_wifi_set_promiscuous(true)` and remains unchanged in later reads.

SOURCE: typed PAC calls in `chips/esp32s31/pac/src/modem/clock_device.rs`,
`chips/esp32s31/hal/src/power.rs`,
`chips/esp32s31/ieee80211/mac/src/init.rs`,
`chips/esp32s31/pac/src/wifi/mac/sniffer.rs`,
`chips/esp32s31/pac/src/wifi/mac/rx/dma.rs` and
`firmware/hil/esp32s31-phy/src/cca_rx.rs`.
Production pulses the Wi-Fi baseband reset at client entry. Its later
`activate_promiscuous_receive` and RX-ring startup configure MAC filters,
enable the RX walker and resume MAC runtime without another baseband
reset. The measurement window starts at a different point in the two
receive sequences. Diagnostic receive leaves the counter configuration
unchanged.
