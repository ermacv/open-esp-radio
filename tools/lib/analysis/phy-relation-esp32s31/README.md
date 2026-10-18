# oer-esp32s31-phy-relation

The reviewed ESP32-S31 PHY calibration relation, shared without copies:

- `committed`: the `phy_param` fields production's committed calibration
  words correspond to, with their byte offsets on both sides;
- `projection` (feature `projection`): production's calibration words in the
  order the vendor comparison reads them, computed from
  `oer_esp32s31_phy`'s validation view.

The comparison probes (`verification/esp32s31/probes`) publish the words, the
Blobray tracking scenario (`verification/esp32s31/scenarios`) compares them
with the pinned vendor code, and the hardware calibration cross-check
(`hil/phy/esp32s31/calibration`) compares a board's capture with
the vendor's. The crate is `no_std` so the probe images link it.

It belongs to the root workspace, alongside the chip's HIL calibration
comparison. Its verification metadata describes this shared checking role;
the production PHY lives in `crates/hardware/esp32s31/phy/`.
