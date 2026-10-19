# oer-esp32s31-knowledge

The reviewed ESP32-S31 PHY calibration relation, shared without copies:

- `committed`: the `phy_param` fields production's committed calibration
  words correspond to, with their byte offsets on both sides.

It is chip knowledge and links no PHY: the radio probe
(`firmware/probes/esp32s31-radio`, its `calibration_projection`) computes
production's words from the PHY state and publishes them, the Blobray
tracking scenario (`tools/vendor/scenarios-esp32s31`) compares them with the
pinned vendor code, and the hardware calibration cross-check
(`tools/vendor/phy-esp32s31`) compares a board's capture with the vendor's.
The crate is `no_std` so the probe images link it.
