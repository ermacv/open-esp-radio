# ESP32-S31 diagnostic PHY capture

`diagnostic-phy` is a separate HIL image. It enables the optional `phy` wire
module, uses public PHY state and typed HAL register reads, and carries no
station role, network stack or verification hooks. Its manifest isolates the
`profile` and `debug-control` cfgs in the diagnostic compile cache.

The diagnostic 802.15.4 path runs first-client tracking before RX and supports
RF close/wake/rejoin, capture and final RF closure. Failed operations retain
INCOMPLETE evidence; a one-pair smoke does not establish vendor spread.

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

The compared Wi-Fi capture prepares descriptor memory, opens the common MAC
core and publishes BASE after cold MAC init, with the walker and MAC runtime
still stopped. Its stopped ring remains owned across capture pages and RF
restart; after the next MAC reset that owner authorizes reopening the core
before the ring is rebuilt and republished. This matches the reference's
core-enabled PHY state without writing noise-floor control or starting RX.
For Wi-Fi, StopCapture first runs a separate CCA experiment after the host's
calibration, MMIO and analog reads. Four placed RX descriptors with 1664-byte
buffers use the same promiscuous activation, walker-enable settle and MAC
runtime resume as W1/W2. Bounded timer-driven polling normalizes completed
single-descriptor frames with W1's ingress decoder, counts parsed beacons on
the capture channel, and returns every unit through the existing HAL. Five
observations are spaced by at least 200 ms; zero beacons rejects the experiment.
No CPU Wi-Fi IRQ route or transmit path is installed. Teardown closes
promiscuous admission and waits for MAC idle before closing the RX gate and
rebuilding the halted ring to reclaim all buffers, then closes RF through the
existing capture teardown. An observation failure still runs cleanup; an idle
timeout quarantines the arena until RF teardown/reset. These later samples are
console evidence only; they do not enter the vendor comparison.
Core preparation and the post-capture experiment also read the existing MAC CCA force
projection, PHY clock-force control, CCA selection and threshold alongside the
counter controls. These are observations, with no counter rearm or enable write.

The image prints typed `TEMPERATURE_SENSOR_SAR2_STATUS` checkpoints after PHY
registration, immediately before RF close and after retained wake. Each line
contains the sampled byte and the named initialization/status field, decoded by
the published PAC field. These reads use the existing PHY clock reference;
there is no read after close with clocks off, no additional power reference and
no analog write. The final capture provides the post-rejoin value. Checkpoints
are outside calibration and tracking-hold measurement brackets.

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

The manual `phy-tracking-hold-ieee802154-idle` investigation runs three boots,
with five due tracking ticks per boot while channel 15 RX remains active.
`StartTrackingHold` saves the registered debug policy and selects `(3, 0)`;
the unchanged production scheduler period and concurrent tracking path run the
RFPLL, common and transmit branches. The image timestamps only Tracking,
grant-protect RFPLL/common/transmit and Wi-Fi baseband-off edges. It retains
bounded raw intervals, including repeated baseband-off intervals, and committed
branch bits. Data is serialized between ticks; no inner-loop clock reads or
per-edge console output are added. A started PHY future is driven to completion.

Stop retires the hold id and restores the saved policy; StopCapture then closes
RF. Overflow, reversed time, failed/missing/duplicate edges, incomplete branches,
cancellation or missing cleanup invalidate evidence. Parent and child intervals
are nested and cannot be summed. Grant-request duration is not proof of RF
exclusion. This image serves only 154 idle hold; associated Wi-Fi hold is absent.
