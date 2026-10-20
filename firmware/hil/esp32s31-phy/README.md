# ESP32-S31 diagnostic PHY capture

`diagnostic-phy` is a separate HIL image. It enables the optional `phy` wire
module, uses public PHY state and typed HAL register reads, and carries no
station role, network stack, verification hooks or instrumentation cfgs.

```console
cargo hil image build diagnostic-phy --chip esp32s31
cargo hil run phy-vendor-calibration-cold --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-restart --chip esp32s31 --board s31-a
cargo hil run phy-vendor-calibration-ieee802154-restart --chip esp32s31 --board s31-a
```

Prepare performs a full registration with no input cache and requires
`full_calibration_performed` and `FullForCache`. It freezes the registration
projection before Wi-Fi startup, matching when the reference prints its
calibration object. The current Wi-Fi home channel is reported by the reference
and supplied explicitly. RX is enabled without association or modem sleep;
no packet traffic is generated.

Indexed MMIO and analog reads run at the selected cold or restarted Wi-Fi/802.15.4 point.
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

The 802.15.4 assembly uses the existing shared MAC engine with channel 15,
21 dBm PIB power, PAN 0x1234, short address 0x0001 and extended address
02:4f:45:52:00:00:00:01, matching the reference. Both automatic ACK paths and
software enhanced ACKs are disabled; there is no transmit command. A private
RX-only laboratory permit has NO_IR; its 21 dBm ceiling preserves the reference
PIB value and does not authorize transmission. The vendor reference retains its
normal automatic ACK policy; these captures compare PHY state in idle RX.
SOURCE: `tools/vendor/phy/src/reference.rs` and the reference peer's CFG command.

The MAC IRQ and all engine entries execute on the same hart at P2, serializing
entries without a task/IRQ lock or a borrow across await. Every received slot is
returned after its callback. Stop silences the CPU route, sleeps the MAC,
returns slots, detaches the quiescent engine, deactivates IRQ ownership and
returns through the registered foundation before leaving the last PHY client.
Rejoin rebinds the same placed storage. The assembly defines no 802.15.4 block
runtime contract. The generic detach/rebind API has no diagnostic-specific shape.
