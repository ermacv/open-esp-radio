# ESP32-S31 BLE PHY register initialization

Source: `esp32s31-bt-lib` `libble_app.a` (pin `10c50778`), member `ble_70.o`
`r_sym_ble_nENHlP4KBuQYlFVffaR5` (role `r_ble_phy_init_registers`) with its
callees (roles `r_ble_phy_ramup_time_set`, `r_ble_phy_monitor_bb_sync`,
`r_ble_hw_resolv_list_set`); from a vendor-code analysis (record outside
this repository). The register evidence is
`registers/esp32s31/evidence/vendor-radio-libraries.toml`
(`BLOB_LIBBLE_PHY_INIT_REGISTERS`).

Facts only. Where a step writes a fixed image, the value is the one in the
reviewed register model, `registers/esp32s31/policy/api.toml`, under the named
operation; it is not repeated here.

## Place in the lifecycle

The body runs after common PHY, baseband, coexistence and Controller software
initialization. Base-stack task enable runs two hardware steps, in this
order:

1. access-address low correlation: baseband `LE_CORRELATION_THRESHOLD_0`
   (`0x2010_2060`) bit 6 `AA_LC_ENABLE` set by a read-modify-write;
2. the PHY register initialization below, with no MMIO access between them.

## Inputs

- Private timing byte: Controller configuration `+0x10`. `r_ble_controller_init`
  writes 91 over the rodata default 90; CCA, which would override it, is off.
  The body uses the byte minus one (written `T` below).
- Environment address: a word-aligned SRAM object. The body publishes the raw
  addresses of members `+0x2c` and `+0x4d`.
- Resolving list: its SRAM address, published in compressed form (bits 19:0).
- Configuration word: Controller configuration `+0x40`, the rodata default of
  `sym_controller_jqcSm1kAtUzAMoyGlaKq` (member `controller_1.o`); nothing
  later writes it.
- Branch bit: set when byte `+0x55` of the `r_sym_ble_Fh7KRrkOJ0DaZapRKpqP`
  options is non-zero; the normal profile leaves it clear.

## Ordered register operations

Every read-modify-write (RMW) reads the register afresh; no software image is
carried between operations on the same register.

| # | Address | Register / field | Kind |
| --- | --- | --- | --- |
| 1 | `0x2010_1ff0` | positional word | complete write of zero |
| 2 | `0x2010_1ff0` | positional word | fixed image |
| 3 | `0x2010_14b4` | `INTERRUPT_ENABLE` | RMW keeping only bit 17 `SOURCE_17` |
| 4 | `0x2010_14b8` | `INTERRUPT_CLEAR` | fixed image: every source |
| 5 | `0x2010_1650` | `LC_TX_ON_DELAY_CONFIG` bits 7:0 and 23:16 | clear |
| 6 | `0x2010_1650` | same two bytes | set both to `T` |
| 7 | `0x2010_2014` | baseband `LE_TX_ON_DELAY` bits 26:19 | replace with `T - 10`; bits 18:16 cleared |
| 8 | `0x2010_1538` | `WORD_0138` | fixed image |
| 9 | `0x2010_18a4` | `WORD_04A4` | fixed byte image |
| 10 | `0x2010_18a8` | `WORD_04A8` | fixed byte image |
| 11 | `0x2010_18a0` | `WORD_04A0` | environment address `+0x2c` |
| 12 | `0x2010_18ac` | `CONNECTION_ABORT_CONTROL` | fixed image |
| 13 | `0x2010_185c` | `WORD_045C` | fixed image |
| 14 | `0x2010_1654` | `WORD_0254` bytes 0 and 1 | clear |
| 15 | `0x2010_1654` | bytes 0 and 1 | set to a fixed value |
| 16 | `0x2010_1654` | bits 22:16 | clear |
| 17 | `0x2010_1654` | bits 23:16 | set to a fixed value |
| 18 | `0x2010_1474` | `WORD_0074` | complete write of zero |
| 19 | `0x2010_891c` | positional word | first fixed image |
| 20 | `0x2010_9800` | accelerator `INIT_CONFIG` | fixed image |
| 21 | `0x2010_98cc` | accelerator `INIT_SRAM_REGION_0` | fixed image |
| 22 | `0x2010_98d0` | accelerator `INIT_SRAM_REGION_1` | fixed image |
| 23 | `0x2010_1880` | resolving list `BASE_POINTER` bits 19:0 | compressed pointer, rest zero |
| 24 | `0x2010_1800` | `WORD_0400` | fixed image (bit 31) |
| 25 | `0x2010_1800` | bit 22 | set |
| 26 | `0x2010_1940` | `WORD_0540` | configuration word |
| 27-29 | `0x2010_1950` | bytes 0, 1, 2 | one byte replacement each |
| 30-33 | `0x2010_1954` | bytes 0 to 3 | one byte replacement each |
| 34-37 | `0x2010_195c` | bytes 0 to 3 | one byte replacement each |
| 38-41 | `0x2010_1958` | bytes 0 to 3 | one byte replacement each |
| 42 | `0x2010_1858` | `WORD_0458` bits 31:16 | replace |
| 43 | `0x2010_194c` | `WORD_054C` bits 4:0 | replace |
| 44 | `0x2010_1870` | `DEVICE_TABLE_CONTROL` bit 18 | set only when the branch bit is set |
| 45 | `0x2010_891c` | positional word | second fixed image |
| 46 | `0x2010_1b74` | `PHY_INIT_CONFIGURATION` | complete image: fixed low byte and bit 8 |
| 47 | `0x2010_1b78` | `PHY_INIT_CONFIGURATION_FOLLOWUP` | fixed image |
| 48 | `0x2010_1244` | Controller `WORD_0244` | complete write of zero |
| 49 | `0x2010_11f0` | `GPIO_DIAG_ARGUMENT_1` | fixed image |
| 50 | `0x2010_1248` | `GPIO_DIAG_CONTROL` | fixed image |
| 51 | modem ETM | event 8 to task 20 | route and enable one channel |
| 52 | `0x2010_124c` | Controller `WORD_024C` | environment address `+0x4d` |

Notes:

- Steps 5 to 7 are `r_ble_phy_ramup_time_set`, which tail-calls the
  external-baseband LE TX-on delay setter.
- Steps 16 and 17 and the zero write of step 18 belong to
  `r_ble_phy_monitor_bb_sync`, which then publishes the first image of
  step 19.
- The byte words 27 to 41 are written in the order `0x550`, `0x554`, `0x55c`,
  `0x558`, one RMW per byte.
- Step 44 is the only condition in the body. The register model places
  `DEVICE_TABLE_CONTROL` at `0x2010_1870`, while the evidence entry names
  `0x2010_1470` for this branch; the address is to be confirmed on hardware.
- The meaning of most of these registers is not established; the register
  model keeps positional names.

## ETM route

The vendor routes modem ETM channel 0 from event 8 to task 20 (`0x2010_880c`
is the event, `0x2010_8810` the task) and enables the channel by writing
`1 << channel` to `0x2010_8804`. In this repository IEEE 802.15.4 owns
channels 0 and 1, so the Bluetooth PHY route uses channel 2; the Bluetooth
runtime owns channels 4 to 7. A Direct Test Mode event without CTE disables
the channel carrying this route, and Test End re-runs PHY initialization,
which routes and enables it again.

## Not established

- The hardware meaning of the fixed images and of the branch bit.
- Whether any write has a latch or ordering requirement beyond program order.
- Failure after the first write has no known rollback; only a complete
  Controller teardown is expected to recover.
