# Memory the hardware reads and writes

Status: proposed (2026-10-18), for the owner's review.

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
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", layout = "WIFI_DMA_DESCRIPTOR" }]
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

`DmaSpace`, `StableDmaRange`, the `.dma.bss.*` sections and the firmware
placement check, unchanged, reading the region bounds from the model.

## Order of work

1. Pilot: the Wi-Fi DMA descriptor and the RX control prefix, both proven on
   hardware by W1. The tool learns the layout kind, `memory.toml` the RAM
   regions; `descriptor.rs` and `prefix.rs` switch to the generated types; all
   their tests keep passing. Between W2's first and second pull requests, so
   W2's TX structures start on the new kind.
2. W2: the TX descriptor words and the A-MPDU descriptors; the offset-named
   fields become `_OPAQUE` fields with their evidence until their meaning is
   known.
3. The second radio block: BLE exchange memory or the 802.15.4 frame buffers.

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
