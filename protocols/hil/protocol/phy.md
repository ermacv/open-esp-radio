# PHY calibration capture wire

The optional `phy` feature serves diagnostic capture only. It is absent from
ordinary images; the host registry includes its message schemas. The framing
and global buffer capacities are the existing HIL ones.

`PrepareCapture` selects Wi-Fi with the reference's observed home channel,
or IEEE 802.15.4 with an explicit channel and provider power. Valid Wi-Fi
channels are 1–14; 802.15.4 channels are 11–26 and power levels are -24 through
21 dBm in 3 dBm steps. Bad configurations are rejected before registration.
`CaptureReady` reports the full cold-calibration path, projection schema,
image lengths, selected radio/channel/power, lifecycle and nonzero capture epoch. The target requires full
registration with no input cache and freezes its output projection before
Wi-Fi startup. Register reads occur later with the selected radio receiving. Lifecycle values
are Cold, Restart, Ieee802154 and Ieee802154Restart. The 802.15.4 comparison uses
register/analog images only; the immutable calibration projection is not read.

`RestartCaptureRadio` requires the active epoch and returns explicit RF-close
and RF-wake evidence plus a new epoch. `StopCapture` closes RF and retires the
active capture. Failed hardware work permits no further capture before reset.

`ReadCalibrationProjection`, `ReadRegisterImage` and `ReadAnalogImage` take an
epoch, first index and nonzero count of at most 16 values. Replies carry epoch,
first index, total image length and bounded values. Every requested range is
validated before access. A reply with missing values, another epoch or changing
image length is incomplete evidence, not a zero-valued observation.

The target exposes only model-indexed register/analog images. The host binds
indices to the model of the archived build and compares production MMIO only at
indices readable on the reference. Calibration values describe the immutable
full-registration product; current temperature and non-retained tracking
progress remain excluded by the existing comparison review.
