# PHY CCA counter: measurement-window ownership

The verdict for `PHY_BB_RECOVERED_GAPS.WORD_0060_OPAQUE`
(`0x20107c60`) is **not established because Blobray's static coverage is incomplete**.
The word remains in the vendor comparison without an exclusion.
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

SOURCE: Blobray at `a19eaad5` (including the caller changes in `f0553471`),
`tools/analysis/cli/src/main.rs::callers` and
`references.rs::{Targets::capture, call_sites, StoredScan}`. Schema-4
`callers` selects relocation references and resolved call/tail-transfer
records, and scans captured data words and exact materialized target addresses.
It reports both ROM calls from `phy_spur_coef_cfg`, agreeing with schema-5
`call-arguments`, as well as the archive call above. Those three sites pass
index 0. The resolved-ROM-transfer omission is absent from this implementation.

SOURCE: the same vendor-calibration ELF's absolute `phy_get_cca_cnt` and
`phy_set_cca_cnt` symbols are `0x2f8015bc` and `0x2f801658`, the ROM export
trampolines. The target lookup accepts these absolute symbols as well as the
ROM's sized function bodies. The query includes both counter names, their
`__call_` export names and the spur-function names. It reports no selected
counter caller and no stored-word or materialized-pointer candidate. The scan
covers seven file-backed sections and 51,482 aligned words, with zero
incomplete inventories; 23 NOBITS sections have no captured contents.

The 10,434 partial functions and four unknown extents still limit absence
claims. Captured pointer candidates would not prove execution, and the
analysis does not expand callees or reconstruct runtime-populated callback
contents. No captured counter-pointer slot was identified whose readers
could establish the next owner. Unknown NOBITS contents are a coverage limit,
not evidence that a callback or the measurement window uses them. Synthetic
linking and ROM address companions do not provide those runtime contents.
The start/finish owner, consumer, subsystem and cadence remain unknown;
neither a vendor-only missing function nor a production defect is established.
