# ESP32-S31 diagnostic PHY capture

`diagnostic-phy` is a separate HIL image. It enables the optional `phy` wire
module, uses public PHY state and typed HAL register reads, and carries no
station role, network stack, verification hooks or instrumentation cfgs.

```console
cargo hil image build diagnostic-phy --chip esp32s31
cargo hil run phy-vendor-calibration-cold --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-restart --chip esp32s31 --board s31-a
```

Prepare performs a full registration with no input cache and requires
`full_calibration_performed` and `FullForCache`. It freezes the registration
projection before Wi-Fi startup, matching when the reference prints its
calibration object. The current Wi-Fi home channel is reported by the reference
and supplied explicitly. RX is enabled without association or modem sleep;
no packet traffic is generated.

Indexed MMIO and analog reads run at the selected cold or restarted Wi-Fi point.
Restart stops the last client, closes RF, wakes it and rejoins, then advances
the capture epoch. The frozen cold-calibration projection remains immutable.
Stop closes RF; a stopped, stale or failed epoch cannot serve pages. A failed
hardware operation poisons the diagnostic service until reset.

The HIL console and dispatcher run on P1. A lock-free RequestSlot transfers each
command to the sole P2 radio owner and returns its response. The console's
bounded input queue is used only on P1. Analog reads retry with timer-backed
waits and a finite observation budget. Register indices come from the published
model, never arbitrary host addresses; the host requests only MMIO indices the
reference could read at the same lifecycle point.

The image uses existing staged boot and chip placement/stack policies. It has
no retained diagnostic post-mortem store; base boot evidence retains the actual
reset reason code. Build audit reports state unresolved stack-call sites.
