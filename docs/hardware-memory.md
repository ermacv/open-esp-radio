# Memory the hardware reads and writes

Status: approved by the owner (2026-10-18).

Some hardware state is not in registers but in RAM: structures whose format
the silicon defines and which the CPU and the hardware hand to each other.
They are part of the hardware just as registers are, and need the same
discipline: one reviewed description, evidence for every field, `_OPAQUE` for
what is unknown, generated accessors, no hand-written masks. This document
proposes how the register model, the PAC and the HAL describe them.

## What exists today

| Structure | Where it is described now | Shared how |
| --- | --- | --- |
| Wi-Fi DMA descriptor (12-byte node: control word, buffer address, next) | `ieee80211/dma/src/descriptor.rs`: hand-written masks and shifts (`SIZE_MASK`, `LENGTH_SHIFT`, owner and done bits) | CPU arms, the walker fills, owner bit 31 hands it over |
| Wi-Fi RX control prefix (metadata the MAC writes at the start of each received buffer: RSSI, PHY info, local timestamp) | `ieee80211/mac/src/rx/prefix.rs`: hand-written byte offsets | hardware writes, CPU reads after completion |
| Wi-Fi TX descriptor words (flags, security, HE control, rate context) | `ieee80211/mac/src/tx/policy.rs`: shifts and fields named by offset (`byte_0x38`, `byte_0x2f_bit_2`) | CPU writes, hardware reads at transmission |
| A-MPDU internal and external descriptors | `ieee80211/dma/src/tx_ampdu_storage.rs` | CPU builds, hardware walks |
| BLE controller exchange memory (packet headers, scheduler lists, CTE ring) | evidence descriptions and `knowledge/esp32s31/bluetooth/` only | CPU and controller both, by protocol |
| IEEE 802.15.4 frame buffers (length byte, PSDU) | `crates/espressif/ieee802154-engine` | CPU and engine |
| The DMA-capable window `0x2f00_0000..0x2f08_0000` | three hand-written copies (`dma/src/descriptor.rs`, `hal/src/ieee802154.rs`, the esp-hal adapter's mem2mem) | placement rule |

None of these pass through the register model, so none have its review,
evidence or naming rules. The hardware facts we measured on the stand (the
walker checks the owner bit, it does not clear it on completion) live in
comments and `knowledge/`, not next to the field they describe.

## How it differs from MMIO

- **No fixed address.** An owner places the structure (a static, an arena, a
  ring) in a region the hardware can reach; the hardware learns the address
  from a register (a descriptor base) or from another structure (a `next`
  link).
- **A placement rule instead of an address.** Size, alignment and the region:
  the DMA window, internal SRAM only, a 4-byte boundary.
- **Shared by protocol.** Each field has a writer (the CPU, the hardware, or
  both at different phases) and a handover: a bit, an order of writes, a
  fence, a doorbell. Registers have access types; these structures have
  phases.
- **Two kinds of access.** While the hardware may touch a word, the CPU uses
  volatile word access. After a proven handover (a completed RX unit), the
  bytes are plain memory the CPU owns.

## Three layers

Same split as for registers: the model says what lies where and who writes
it; the HAL says in which order and who owns it.

### 1. Layout: the register model and the generated PAC

A new kind of model entry, a **memory layout**, beside peripherals:
`registers/<chip>/model/layouts/<name>.toml` (and in a register library when
several chips share it, as `registers/ieee80211/` does for MAC blocks).

```toml
[layout]
name = "WIFI_DMA_DESCRIPTOR"
description = "The 12-byte node the Wi-Fi DMA walker follows."
size = 12
align = 4
placement = ["dma-window"]          # regions from model/memory.toml

[[layout.words]]
name = "CONTROL"
offset = 0
access = "shared"                   # volatile while the hardware may touch it

[[layout.words.fields]]
name = "OWNER"
bits = "31"
writer = "cpu"                      # the hardware reads it, never clears it
description = "1: the walker may fill this node; it stops at a node with 0."

[[layout.words.fields]]
name = "RX_DONE"
bits = "30"
writer = "hardware"

[[layout.words.fields]]
name = "BIT_29_OPAQUE"
bits = "29"
writer = "cpu"
description = "Cleared when the node is rearmed; meaning unknown."

[[layout.words.fields]]
name = "LENGTH"
bits = "27:14"
writer = "hardware"

[[layout.words.fields]]
name = "SIZE"
bits = "13:0"
writer = "cpu"

[[layout.words]]
name = "BUFFER"
offset = 4
access = "shared"
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", placement = ["dma-window"] }]

[[layout.words]]
name = "NEXT"
offset = 8
access = "shared"
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", points-to = "WIFI_DMA_DESCRIPTOR" }]
```

Byte-oriented layouts (the RX control prefix) declare `[[layout.bytes]]`
ranges instead of words, with `access = "owned"`: the CPU reads them only
after a handover.

Every field gets a `[[review]]` like a register field: naming, sources,
provenance, accuracy, completeness. A fact measured on the stand is evidence
of a new class, `stand`, citing the `knowledge/` note and the runs; vendor
facts keep their `BLOB_*` ids.

`memory.toml` gains the RAM regions the layouts name (`dma-window`,
`internal-sram`), so the DMA window has one source instead of three.

The generator emits, in the chip's PAC (`generated.rs`, not the raw PAC,
since svd2rust has no notion of an unplaced layout):

- a `#[repr(C, align(N))]` type of the layout's words, `Zeroable`, with
  compile-time size and alignment checks;
- for each `shared` word, a cell with volatile `read() -> Value` and
  `write(Value)` only; for each word a `Value` type with typed getters and
  `with_*` builders, as the PAC's register values have;
- for `owned` byte layouts, a decoder from `&[u8; N]` (fixed-size arrays, no
  panics), the shape `prefix.rs` already follows by hand;
- the placement as constants the HAL checks against (`StableDmaRange`,
  `placement.rs`).

No protocol logic in the PAC: no ordering, no fences, no ownership.

### 2. Protocol: the HAL

Hand-written and typed, as today in `rx_ring.rs`, `rx_storage.rs` and the TX
storage: the ownership machine (armed, completed, detached, rearmed), the
publication order (buffer address, then `CONTROL` with `OWNER = 1`, a fence,
the doorbell), the proofs (`RxDmaWalkerStopped` naming its arena), the
reserved descriptor behind `BASE`. The HAL reaches the structures only
through the generated types; `CLAUDE.md`'s rule against local masks extends
to hardware-shared memory.

### 3. Placement: platform and HAL

A DMA static is declared with `oer_esp32s31_hal::dma_static!`: a zeroed
`DmaStatic<T>` in a `.dma.bss.<block>` section (the macro refuses any other
section name), claimed once as a `Placed<T>`. The register generator writes
`firmware/platform/esp32s31/linker/dma-window.x` from the model: the
`dma-window` region and the width of `RX_NEXT_DESCRIPTOR.ADDRESS_LOW`. The
runtime linker script includes it and asserts that all DMA storage lies in
the window and holds no address whose low address bits are zero; the
runtime link uses `--orphan-handling=error`, so a mistyped section name
cannot leave a static unplaced. A storage that publishes its own addresses
binds from a `Placed` value (`WifiRxArena::bind`); host models use
`Placed::host` (test support). `DmaSpace` and `StableDmaRange` stay. The
run-time address checks and the firmware placement warning go with the HAL
RX DMA engine (step 3); `cargo fw` refuses an image that enables
`test-support`.

A transmit pool is placed the same way: `PlacedTxPool::pin` takes a
`Placed<PinnedDmaTxPool>`, and its radio leases are `PlacedTxLease`s. The
sealed `PlacedBacking` marks them, and the pool's wrappers around them, as
buffers in the DMA window; the A-MPDU retention takes only `PlacedBacking`s and
publishes their addresses without a window check. A buffer lent from outside
placed storage has no source yet; it returns as a loan type, checked once where
the loan is made, with the first such buffer.

## Where registers, layouts and the protocol meet

Approved by the owner (2026-10-18).

- **One IP block, one description.** A layout belongs to the peripheral that
  consumes it, beside its registers. A register field the hardware reads as
  a pointer names its target in the model (`points-to =
  "WIFI_RX_DMA_DESCRIPTOR"`), and the PAC accessor then takes a typed
  reference (`DmaRef<WifiRxDmaDescriptor>`), never a bare `u32`.
- **The protocol is that block's HAL engine.** One HAL type owns both the
  block's register token (from the ownership root) and its placed arena; the
  base register, the doorbell and every descriptor publication are its
  methods. Neither the registers nor the arena is reachable apart from it.
  Drivers above see leases, cursors and outcomes, never addresses.
- **Placement is proved before run time.** An arena is declared through a
  macro that puts its static into the block's `.dma.bss.<block>` section; a
  linker `ASSERT` keeps the section inside the DMA window and off a base whose
  low 20 bits are zero. `DmaRef<T>` is built only from such a placed static,
  and buffer addresses are typed offsets inside the same arena, so they are
  valid by construction. Size and alignment are compile-time checks. The
  run-time address checks (`descriptor_address_valid`, the run-time part of
  `placement.rs`, the firmware placement warning) are removed. What the
  hardware writes back (`NEXT`, `LAST`) is outside input and stays checked,
  against the arena rather than the window. Host tests keep the `HostDma`
  space with the same types.
- **The unsafe boundary.** Three narrow places, all in the PAC and the HAL:
  the generated volatile word accesses (a safe API over plain words); the one
  placement constructor (justified by the linker assertion and a single
  `take`); and the protocol core, where a CPU view of a buffer is created
  after the completion proof and where a buffer is handed back to the
  hardware. Crates above the HAL carry `#![forbid(unsafe_code)]`.

## Order of work

1. Pilot: the Wi-Fi DMA descriptor and the RX control prefix, both proven on
   hardware by W1. The tool learns the layout kind, `memory.toml` the RAM
   regions; `descriptor.rs` and `prefix.rs` switch to the generated types; all
   their tests keep passing. During the predecessor port, before the
   ESP32-C5 HAL and W2's TX structures, so both start on the new kind.
2. Typed links, then within the same port wave: `points-to` in the model and
   typed references in the PAC; placement through the macro and the linker
   assertion; the RX DMA engine in the HAL as the only owner of its registers
   and arena; the run-time address checks removed; `forbid(unsafe_code)` in
   the driver and above.
3. W2: the TX descriptor words and the A-MPDU descriptors; the offset-named
   fields become `_OPAQUE` fields with their evidence until their meaning is
   known.
4. The second radio block: BLE exchange memory or the 802.15.4 frame buffers.

## Open questions

- **Home of a layout shared by chips.** The Wi-Fi descriptor is likely common
  to several Espressif chips. We are S31-only, so the pilot keeps it in the
  chip model; it moves to a library when a second chip appears.
- **SVD and the binding index.** A layout has no address, so it does not go
  into the SVD. Whether vendor analysis needs it in the binding index is
  decided when binary analysis returns.
- **Phase per field.** `writer` names one writer. Fields written by the CPU in
  one phase and by the hardware in another (none known yet) would need a
  phase list; added only when one appears.
