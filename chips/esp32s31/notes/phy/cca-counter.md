# PHY CCA counter: measurement-window ownership

The verdict for `PHY_BB_RECOVERED_GAPS.WORD_0060_OPAQUE`
(`0x20107c60`) is **not established because of Blobray's caller-analysis
limits**. The word remains in the vendor comparison without an exclusion.
Neither a missing vendor-only service nor a production CCA defect is proved.

## Register facts

SOURCE: pinned ESP32-S31 rev0 ROM ELF, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`;
complete `phy_set_cca_cnt` at `0x2f8256cc`, size `0x2e`, and
`phy_get_cca_cnt` at `0x2f8256fa`, size `0x24`.

| Address | Function | Effect |
| --- | --- | --- |
| `0x20107c58` | `phy_set_cca_cnt` | Replaces bits 26:0 from arg0 (`0x07ffffff`), preserving bits 31:27; nonzero arg1 additionally sets bits 28:27 (`0x18000000`) in a fresh read-modify-write. |
| `0x20107c5c` | `phy_get_cca_cnt` | Copies bits 26:0 to out[0] and returns bit 27 as 0/1. |
| `0x20107c60` | `phy_get_cca_cnt` | Copies bits 26:0 to out[1]. |

These leaves do not establish the hardware meanings of the control bits,
who starts or finishes a window, the result consumer, its subsystem or its
invocation period. Production PHY/HAL/driver source has no explicit
counter-window arm operation or call of the ROM setter. The diagnostic
`cca_checkpoint.rs::observe_running` only reads the selected words through
`RadioDomain::phy_register_image`. An undocumented hardware trigger is not
excluded by this source finding.

SOURCE: s31-b captures `1792535094189-0008ca74` and
`1792535695644-000980e7`, channel 1 with router beacons. The diagnostic received
56 and 63 parsed same-channel beacons respectively, but all five open-RX
samples per capture read zero at `+0x60`; the paired vendor captures read 4.
Both sides read `+0x58=0x80010000` and `+0x5c=0x08010000`. Reception alone
therefore does not resolve the window-ownership question.

## Analysis coverage

SOURCE: the 15 S31 archives of IDF
`4d59230ddff16327812782151ef0afef202dc6d7`, the ROM above, and the built
vendor-calibration ELF, SHA-256
`7e609cb94b84527a5ed4f12adee77a0321ef297b4799b4da5f590ffb75eb6078`;
application SHA-256
`e7c115dedfdb1fbf65372ef18655855b2f7d5b415f00de5a2d8347fc0940272a`.
Blobray register-access schema 8 with the integer ABI, call-return constants
and the explicit flash read-only window reports exact numeric target accesses
only in the two ROM leaves. This is not proof that no other access exists:
13,801 functions were analyzed, 10,434 were partial and four had unknown
extents. There were 236,644 nonnumeric-address observations, including
ordinary stack and buffer accesses.

SOURCE: ROM `phy_spur_reg_write_one_tone` at `0x2f829a5e`, size `0x25e`.
Its unresolved indexed addresses include `0x20107c14 + 4*arg0`, which could
reach the counter words at indices 17/18/19. The observed call in
`librftest.a[wifi.o]::set_spur_reg` and two resolved ROM calls from
`phy_spur_coef_cfg` pass index 0; they do not establish counter access.

SOURCE: `tools/analysis/cli/src/main.rs::callers` and
`tools/analysis/cli/src/listing.rs::references_to`, as analyzed at
`50970d8d5336e07c2c14bcc728e56ff76158f457`. `callers` selects relocation
references by target name. It does not build caller chains from resolved
ET_EXEC/ROM transfers, standalone data-table references or runtime callbacks.
Concretely, `call-arguments` reports the two ROM spur calls above, while
`callers` omits them. Neither query reports a selected site for the counter
leaves or their export names; this does not prove the absence of indirect
callers. Synthetic linking and ROM address companions do not supply that
missing traversal. The owner and cadence of the CCA window remain unknown.
