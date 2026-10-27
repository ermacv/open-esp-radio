# ESP32-S31 IEEE 802.15.4 static policy and dataplane contracts

This reference describes public ESP-IDF source contracts pinned at commit
`4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`. Source order, software state and
hardware readiness are distinct claims. Reviewed register semantics belong to
[the register model](../registers/README.md); production
implementation retains its own authority.

## Source ledger

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/ieee802154/esp_ieee802154.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/esp_ieee802154.c#L174) | ACK-timeout conversion, primary identity, frame-length check | `bfa12943c503eb9c56e7774faccb9429d6f26e02db0b82afb2c7fdfe1752fd7a` |
| [`components/ieee802154/include/esp_ieee802154.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/include/esp_ieee802154.h#L455) | TX frame and ACK-timeout public contract | `26c21c7f45d7e5f1ec2527cfd0182434d2348a0475c9dfc2292481493b2f2477` |
| [`components/ieee802154/include/esp_ieee802154.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/include/esp_ieee802154.h#L455) | RX buffer layout and replacement of FCS by RSSI/LQI | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_pib.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_pib.c#L75) | opaque TX-power conversion and static PIB write order | `4bc94779b0c29fdfc77dcdf0c6d3d66fad5d02324aa951d9f19877bc62532cf4` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L265) | event RMW and direct RX/TX DMA address writes | `ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471` |
| [`components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h#L265) | frequency, timeout, primary identity, CCA fields | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1039) | 128-byte RX storage, processing lease, stub buffer | `517e0c761e5f01b3bf1797bbbbc54a16d2a9e4b79d8e32a345124d9bc6e4b9e4` |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1039) | RSSI/LQI indices, next-buffer selection, stub fallback | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1039) | ISR order, init masks, IRQ allocation | same file/hash as above |
| [`components/ieee802154/driver/esp_ieee802154_dev.c`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/driver/esp_ieee802154_dev.c#L1039) | stop, PIB update, address publication, command start | same file/hash as above |
| [`components/ieee802154/private_include/esp_ieee802154_frame.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/ieee802154/private_include/esp_ieee802154_frame.h#L17) | inclusive PHR range `3..=127` | `c4d326c59bd71a43db2de265ab3886064faf95ee018a541fc2deb0bc5609d1e1` |
| [`components/soc/esp32s31/include/soc/soc.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/include/soc/soc.h#L169) | DMA-capable internal address interval | `b19cf9e6916f1416b8385160c6de277ff64f8cd80e9cb22d44833d9f9654e92b` |
| [`components/soc/esp32s31/register/soc/ieee802154_struct.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/register/soc/ieee802154_struct.h#L324) | direct TX/RX address words at offsets `0xd0`/`0xe0` | `da13c2bc78cd6ef35a4e54ddddf11ce48fda967746193f1a0ad03578a5881752` |
| [`components/soc/esp32s31/register/soc/ieee802154_reg.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/register/soc/ieee802154_reg.h#L318) | DMA register geometry | `fd3f944ac97634605083031f96c0f942af26a81a9e9a3123281c59e5719f9d9c` |
| [`components/soc/esp32s31/include/soc/interrupts.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/include/soc/interrupts.h#L16) and [L139-L153](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/include/soc/interrupts.h#L16) | source table and ZB MAC source `132` | `1a4f155b87090376b1a40ac62e19de344c7f10dc53d9b4451b66d545e9e4717d` |
| [`interrupt_core0_reg.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core0_reg.h#L2786-L2805) / [`interrupt_core1_reg.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/soc/esp32s31/register/soc/interrupt_core1_reg.h#L2786-L2805) | per-core route fields | `60785a48a2b2be35670a789bf2e0a82a45f4bd85f110fc18c197de14211f311e` / `e7966b5cda22b0df047182c6f7758c385b95edb991cdc40d0794127fdfdaf335` |
| [`components/soc/esp32s31/register/soc/reg_base.h`](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/soc/esp32s31/register/soc/reg_base.h#L137) | core-one interrupt block is core zero plus `0x800` | `24a0c98ca63b042ed32d42c5566b979d412e04ec7b1b9d305bb7addc12577f9d` |
| [`components/esp_hw_support/include/esp_intr_alloc.h`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/esp_hw_support/include/esp_intr_alloc.h#L135-L169) | flags-zero allocation class and calling-core ownership | `c701554081a0a3855244209f41873dfdb0ddae542a1afb4104d3bcf03bd4c566` |

## 1. Static MAC-policy subset

The public source supports a bounded static policy without claiming the full
vendor PIB:

- channel `11..=26`, converted to the MAC frequency code;
- the four CCA modes and the complete signed `int8_t` threshold domain;
- TX auto-ACK, RX auto-ACK, enhanced-ACK TX, coordinator, promiscuous, and the
  one-bit enhanced-pending projection;
- the primary PAN ID, short address, and eight-byte extended address;
- ACK timeout in 16-microsecond field units.

The public ACK setter computes `(microseconds + 15) / 16`; the getter multiplies
the field by 16. A checked Rust conversion must perform the addition in a wider
integer and reject an input whose rounded result exceeds `u16::MAX`. It must not
reproduce the C `uint32_t` addition overflow at the extreme end of the input
domain.

The extended address is packed little-endian into two words. Each primary
identity setter enables multipan context zero while preserving other context
bits. Readback must compare typed fields, including the context-zero enable
bit, rather than compare complete register images.

The source-backed PIB suborder is frequency, TX power, CCA mode, threshold,
three ACK controls, coordinator, promiscuous, and pending mode. TX power
converts dBm through the level set returned by `bt_bb_get_tx_pwr_table()`.
The HAL ports the public clamp-and-floor scan over a non-decreasing level set;
it rejects an empty set, a length outside the public eight-bit domain, or
descending values before producing a channel-bound opaque field code.
`ESP32S31_TX_POWER_LEVELS` is the provider's set recovered from the
vendor library: sixteen levels from -24 to 21 dBm in 3 dB steps, whose index
is the `TXPOWER` field code. The level set is not an RF calibration result.
The static policy transition itself writes no TX-power code.

All event and abort masks remain zero before, during, and after this static
policy transition. A write fence followed by one typed snapshot provides a
fail-closed publication boundary. A policy-only mismatch retains the preceding
foundation owner for an exact retry; a mask, ED-sampling, or PTI mismatch
invalidates that proof and demotes ownership to the preceding reset state.

## 2. Direct frame-storage contract

This MAC takes one direct 32-bit TX address and one direct 32-bit RX address.
The reviewed operational path does not construct a GDMA descriptor ring for
these transfers. That does not prove every DMA status/configuration register;
only the direct-address contract is required by this slice.

The PHR length is inclusive `3..=127` and includes the two-byte FCS. A fixed
128-byte TX object can therefore represent:

```text
byte 0              PHR = MAC length + 2
bytes 1..PHR-2      MHR and MAC payload
bytes PHR-1..PHR    reserved/zeroed FCS positions; hardware generates FCS
remaining bytes     zeroed, never published as frame content
```

The MAC input length is consequently `1..=125` bytes. The source does not
require software FCS bytes; explicitly reserving and zeroing their positions
prevents stale bytes from becoming observable if a later hardware contract
widens the fetch.

For a valid receive:

```text
byte 0              PHR
bytes 1..PHR-2      MHR and MAC payload, no FCS
byte PHR-1          RSSI code (`i8`)
byte PHR            LQI (`u8`)
```

The whole fixed object fits at the maximum PHR because the last occupied index
is 127. A parser must reject PHR values outside `3..=127` before forming any
slices.

The S31 public memory map declares the half-open interval
`[0x2f00_0000, 0x2f08_0000)` DMA-capable. Address validation must cover the
entire 128-byte object, not only its first byte, and must reject truncation from
`usize` to the 32-bit hardware field. Four-byte alignment is a conservative
open-driver storage invariant, not a newly claimed hardware minimum.

## 3. RX ownership and exhaustion

Every vendor RX slot has a software `process` lease. Hardware may receive into
a slot only while that flag is false; delivery sets it true; the upper layer's
handle-done call validates that the returned pointer belongs to the pool and
releases exactly that slot. Reusing a delivered buffer before release would
violate the public lifetime contract.

When every valid slot is leased, the vendor programs one extra fixed stub
buffer. A frame received there is dropped rather than delivered. A safe pool
therefore needs the explicit path:

```text
Free -> Armed -> Delivered -> Free
                 all busy -> Stub (never delivered)
```

The hardware-address token and the storage lease must be non-forgeable outside
the DMA leaf. A device-ordering fence is required before publishing an armed
buffer and before making completion-visible bytes available to task code.

## 4. IRQ ownership and affine acknowledgement

The peripheral interrupt source is exactly `132`. Both cores have a dedicated
mapping word at route offset `0x210`; the CPU interrupt number occupies bits
`5:0`, and pass/remap level occupies bits `9:8`. Vendor allocation uses flags
zero, so the CPU vector is selected dynamically on the current core and must
not be hard-coded.

Vendor initialization first enables the raw fourteen-bit event image and then
masks timer zero, leaving `0x3eff`. That image still includes unnamed bits 7
and 13 plus named-but-unhandled clock-count-match bit 10. The quiesced open
plan therefore uses the fail-closed intersection `0x3eff & 0x1b7f = 0x1a7f`;
it does not present that safe subset as the exact vendor register image. The
RX/TX abort masks remain the exact reviewed initialization images.

The vendor ISR snapshots event status and both abort reasons, acknowledges the
snapshot, then dispatches in this order:

1. RX-abort phase one;
2. RX SFD, TX SFD, TX done, RX done, ACK TX, ACK RX;
3. RX-abort phase two, TX abort, ED done, timer 0, timer 1;
4. one deferred `next_operation` decision.

That order remains represented and exhaustively tested as pure logic, and the
production path connects it to the HAL's interrupt owner. The target
reviewed fact assigns read-write W1C semantics to `EVENT_STATUS`; the published
SVD carries `oneToClear`, and the generated code samples the complete
fourteen-bit field into a non-`Copy` token consumed by acknowledgement. No
caller can supply, narrow, clone, or replay the write image.

The HAL splits the PAC's generated MAC block into the task owner
`Ieee802154TaskOwner` and the inactive `Ieee802154InterruptSetupOwner`.
Activation masks delivery, consumes one complete stale W1C snapshot, publishes
the named runtime event mask, and returns a disjoint `Ieee802154InterruptOwner`. Its hard-IRQ transaction samples
the complete event image plus selected RX/TX abort and ED/CCA sidebands before
consuming the exact W1C token. A production port adapter posts the resulting
non-replayable value to the bounded Embassy queue; task code never receives
status-register authority.
The platform adapter owns source-132 routing and its teardown around the
prepared interrupt owner. Same-bit arrival and level-line retrigger do not
change the W1C access contract; a hardware route probe answered both on the
ESP32-S31 over the production owners and route adapter: a second arrival of an event whose bit
is still latched is cleared with the first (the arrivals coalesce, as the
vendor's snapshot-then-clear ISR also assumes), and an event latched after the
ISR's snapshot retriggers the level-driven source-132 route once the snapshot
is consumed.

## Ownership boundary

Frame storage remains pinned while hardware owns its direct address. The
control runtime retains RX/TX owners until an accepted acknowledged batch
authorizes reclamation. The IRQ owner alone samples and acknowledges status;
the bounded async handoff carries values, not PAC capabilities or raw DMA
addresses. Queue overflow fails closed. `STOP` is not synchronous completion
evidence. Whole-radio readiness belongs to composed PHY/BTBB/coexistence,
platform and MAC owners.
