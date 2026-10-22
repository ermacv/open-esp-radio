# ESP32-S31 BLE controller memory

The reviewed layouts describe the physical RAM format, independently of
which CPU owner currently holds it. The allocation, initialization and
handover facts come from `esp32s31-bt-lib` pin
`10c507788e9da0993709cf82e405c896561172d8`, `libble_app.a` SHA-256
`e61c5f8b0e558df8c520bcedd78dd8b930c2b61350b08c03430b774275385723`.
The evidence catalog is `registers/evidence/ble-memory.toml`. Unknown
physical meanings remain `_OPAQUE`, with `accuracy = "positional"`, only where a cited use is observed. Preserved lanes and zero-only allocation positions keep their exact storage image without field accessors.

## Buffer headers and packet views

The RX, TX and DTM headers have the same six-word physical geometry. A
compressed next-header link occupies the low twenty bits of word zero.
Word one holds a compressed byte-buffer base, including its RX/TX prefix.
For TX, word two additionally holds the compressed packet `+0x10` byte
address. Its high lanes distinguish the exact RX and TX allocation images;
their physical meanings are not established. Word three's bit 31 is the
completion gate. Word four carries the software terminal marker and TX
extent/completion metadata. Word five is a full CPU predecessor address.
The complete RX/TX allocators in `memMgmt_3.o` and the receive-recycling
bodies recorded in [the scheduler reference](../bluetooth-scheduler-lists.md)
establish those positions.

The RX and DTM RX prefix is `0x1e` bytes; the TX prefix is `0x12` bytes.
These are byte buffers, rather than links to structures: even a pointer to
their first prefix byte takes `DmaWindowAddress`. Their owned decoders are
used only after the header completion protocol has returned ownership.
The RSSI byte at RX `+0x0f` is signed. DTM's callback accepts only a zero
low-24-bit result at `+0x0c`; the individual rejected flags and the physical
unit of its RSSI remain unassigned. The captured word at `+0x10` has no
independently established epoch or on-air-time interpretation in the layout.
The low halfword at `+0x18` is armed with an epoch sentinel; the current
codec compares its produced value with its allocation tag. That comparison
does not establish an independently measured hardware tag identity.

## Link states and scheduler items

All five role graphs use the same `0x84`-byte physical link-state geometry.
Advertising and connectable advertising share the same reset-word projection;
DTM, scanning and connection select different values in the same lanes.
Word `+0x18` is mode-dependent: scanner policy flags, or a whole controller
receive-time image in connection mode. Its scanner fields are not
independent connection timestamp fields. The same qualification applies to
connection octet-limit lanes used by other reset profiles as opaque images.

The properties at software `advsm+0x0c` (connectable, scannable, directed,
high duty cycle and legacy) select the graph. They belong to the separate
software advertising object, not to hardware link-state `+0x0c`; see
[scannable and directed sets](../bluetooth-legacy-advertising.md#scannable-and-directed-sets).

The common `0x60`-byte scheduler item has a compressed execution successor
at `+0`, a compressed separate `0x48`-byte context at `+4`, and a compressed
link-state address at `+8`. Its previous and completed-queue links at
`+0x50/+0x54`, and the software link-state anchors at `+0x64..+0x78`, are
full CPU addresses. They must not be converted to compressed links.
Execution status is phased: CPU initialization with `0xffffffff`, then a
hardware result; CPU preemption may write zero. The role sources and exact
opaque images are recorded in the [scheduler](../bluetooth-scheduler-lists.md),
[DTM](../bluetooth-direct-test-mode.md),
[advertising](../bluetooth-legacy-advertising.md),
[scanning](../bluetooth-passive-scanning.md) and
[connection](../bluetooth-peripheral-connection.md) references.

## Direction-finding workspace on the current pin

Complete `ble_4.o::r_sym_ble_mIskG2GvbtwJXrhsY5EZ` is 60 bytes. It calls
`ble_35.o::r_sym_ble_pzoiUAZUrqXUUY6RB1c3` with length `0x1c`, count one,
and flags zero/one. The complete latter body multiplies length by count,
allocates and clears that extent on this branch. On success the initializer
stores byte one at `+8` and word one at `+0x0c`.

Complete `ble_4.o::r_sym_ble_FZU452JXpwl3LtErzDPJ` stores the compressed
workspace `+8` endpoint in link-state `+0x50`. If the first separate
sample-buffer pointer is zero, it publishes workspace `+0x0c` through slot
zero using `r_sym_ble_0wJYOi1maT14CoxXAvPQ`, then calls
`r_sym_ble_FP0qQtIrPdb7A8lmBaH3(0)` to clear software ownership. It selects
six hardware slots through `r_sym_ble_jvAPXyi7YenxwkkHNdZq(6)`, then
updates link-state halfwords `+0x52/+0x56` to the disabled ordinary-role
policy. These are current pinned bodies, rather than a transfer of the
older archive's behavior.

The open memory owner retains `0x20` bytes. Its last word is retained
padding; this does not assert a `0x20`-byte allocation in the pinned
initializer. The generated workspace preserves that extent. Separate
configuration and descriptor layouts describe the referenced members at
`+8` and `+0x0c`. Their unassigned words retain storage without bit accessors. The six hardware
slots do not make this workspace a software CTE ring.
