# PHY calibration capture wire

The optional `phy` feature serves diagnostic capture only. It is absent from
ordinary images; the host registry includes its message schemas. The framing
and global buffer capacities are the existing HIL ones.

`PrepareCapture` selects the reference's observed Wi-Fi home channel.
`CaptureReady` reports the full cold-calibration path, projection schema,
image lengths, lifecycle and nonzero capture epoch. The target requires full
registration with no input cache and freezes its output projection before
Wi-Fi startup. Register reads occur later, with Wi-Fi RX enabled.

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
