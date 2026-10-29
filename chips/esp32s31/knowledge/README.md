# ESP32-S31 chip knowledge

The reviewed ESP32-S31 PHY calibration relation, shared without copies:

- `committed`: the `phy_param` fields production's committed calibration
  words correspond to, with their byte offsets on both sides.

It is chip knowledge and links no PHY: the radio probe
(`firmware/probes/esp32s31-radio`, its `calibration_projection`) computes
production's words from the PHY state and publishes them, the Blobray
tracking scenario (`tools/vendor/scenario/src/esp32s31`) compares them with the
pinned vendor code, and the hardware calibration cross-check
(`tools/hil/family-phy-esp32s31`) compares a board's capture with the vendor's.
The crate is `no_std` so the probe images link it.

## Semantic calibration record

`calibration_cache` is the hardware-free codec for `S31SemanticV1`, format
identifier `0x31433353` (little-endian ASCII `S3C1`). It preserves every field
of PHY snapshot schema 8 in exactly 261 bytes. The firmware maps fields
explicitly between this record and its native snapshot. The codec depends
on neither PHY nor HAL and does not authorize replay.

| Part | Bytes |
| --- | ---: |
| Schema (`u16`) | 2 |
| Identity: RF ABI (`u32`), base MAC (`[u8; 6]`), extension (`u16`) | 12 |
| Common calibration | 32 |
| Wi-Fi calibration | 182 |
| Bluetooth calibration | 33 |

Field declaration order in `calibration_cache/mod.rs` is wire order. Arrays
are row-major. Integers are fixed-width little-endian; signed integers use
two's complement; booleans are exactly 0 or 1. There is no padding, native
pointer, enum discriminant or `usize`. `golden.bin` and its field-complete
semantic fixture fix that encoding independently of Rust memory layout.
Decode rejects unknown formats/schemas, any wrong length and noncanonical
booleans. Foreign identities, incomplete guards and invalid RX table values
are preserved for the native registration boundary to reject before replay.

`CALIBRATION_ALGORITHM_REVISION` and `CALIBRATION_PROFILE_ID` identify the
reviewed calibration/replay implementation and `PhyConfig::production()`.
They are independent of RF ABI 102, snapshot schema and producing commit.
A change to how PHY calibration computes a snapshot field, or to how
replay applies it, takes a new revision/profile pair; previously used pairs
are never reused. The device and the host admit only a record of the
image's own pair and reject any other before replay. A PHY change that
alters neither keeps the pair.
