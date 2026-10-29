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

`StartCalibration(Full)` requests a full registration with no input cache and requires
`full_calibration_performed` and `FullForCache`. It freezes the registration
projection before Wi-Fi startup, matching when the reference prints its
calibration object. The current Wi-Fi home channel is reported by the reference
and supplied explicitly. RX is enabled without association or modem sleep;
no packet traffic is generated.

`HostCalibrationCache` advertises the manual boot-scoped cache service.
Identity lookup is read-only. `StartCalibration(HostCache)` opens one 500 ms
window, receives sequential pages of `S31SemanticV1`, and seals a candidate,
explicit miss, rejection or timeout before native registration. Native identity,
completion and RX-table checks decide replay; caller admission and actual native
path are reported separately. A failed hardware registration stays poisoned.

The sole P2 radio owner keeps a bounded inbox, reuses it for an immutable export
after successful full registration, and does not export a new record after
partial calibration or retained RF wake. P1 reaches it through RequestSlot and
waits for the initial/sealing answer to reach the endpoint before releasing registration. A sealed
operation cannot wait past the original deadline for console publication.
Calibration temperature is reported in degrees Celsius, as the native snapshot
defines it. The [host store](../../../tools/hil/session/CALIBRATION_CACHE.md) owns
persistence; the firmware has no calibration flash/NVS backend.

`cargo hil run phy-host-calibration-cache --chip esp32s31 --board s31-a` exercises
three cold full/save/reset/partial pairs, retained RF wake after partial, and
separate missing-file/no-response full starts. It records native registration
microseconds, host file-load/transfer/start durations, boot identities and the
full producing record in run artifacts. Existing vendor comparisons and tracking
holds request and validate `Full` explicitly.

Indexed MMIO and analog reads run at the selected cold or restarted Wi-Fi/802.15.4 point.
Restart stops the last client, closes RF, wakes it and rejoins, then advances
the capture epoch. The frozen cold-calibration projection remains immutable.
Stop closes RF; a stopped, stale or failed epoch cannot serve pages. A failed
hardware operation poisons the diagnostic service until reset.

The restarted Wi-Fi capture prepares descriptor memory, opens the common MAC
core and publishes BASE after rejoin, with the walker and MAC runtime still
stopped. Its stopped ring remains owned across capture pages and later RF
restarts; after the next MAC reset that owner authorizes reopening the core
before the ring is rebuilt and republished. This matches the reference's
core-enabled restart state without writing noise-floor control or starting RX.
Cold capture retains the gated-core observation point and prepares the ring
only in StopCapture, after the host's reads.
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

Startup publishes the executor's system counter before drawing the boot identity.
The RNG's inter-read settle uses that advancing clock even before executors start.

The P2 owner pins large child futures and awaits references to their retained
storage. Owned registration, maintenance and RF results are polled into parent
storage; a separate completion frame returns domain custody and inspects the
registration result in place before diagnostics or capture bring-up.
Future construction and request handling have separate non-inlined frames, so
their scratch does not remain live beneath calibration or tracking polls.
Owned PHY polls use the existing PHY `Boundary` around pinned references, so
each operation's scratch stays in its own poll frame. MAC event polling has
its own frame as well.

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
SOURCE: `tools/hil/family-phy/src/reference.rs` and the reference peer's CFG command.

The MAC IRQ and all engine entries execute on the same hart at P2, serializing
entries without a task/IRQ lock or a borrow across await. Every received slot is
returned after its callback. Stop silences the CPU route, sleeps the MAC,
returns slots, detaches the quiescent engine, deactivates IRQ ownership and
returns through the registered foundation before leaving the last PHY client.
Rejoin rebinds the same placed storage. The assembly defines no 802.15.4 block
runtime contract. The generic detach/rebind API has no diagnostic-specific shape.

The manual `phy-tracking-hold-ieee802154-idle` and
`phy-tracking-hold-wifi-idle` investigations run three boots, with five due
tracking ticks per boot while channel 15 IEEE 802.15.4 RX or channel 1 Wi-Fi
remains active. Wi-Fi marks the retained temperature band stale before each
tick; the ordinary T3 child then performs both masked writes and commits the
actual band. Engine reports use family 3 for this child, alongside T1 and T7.
`StartTrackingHold` with `policy = Forced` saves the registered debug policy and selects `(3, 0)`;
the unchanged production scheduler period and concurrent tracking path run the
RFPLL, common and transmit branches. The image timestamps only Tracking,
grant-protect RFPLL/common/transmit and Wi-Fi baseband-off edges. It retains
bounded raw intervals, including repeated baseband-off intervals, and committed
branch bits. A root-future wrapper also counts polls and directly measures Pending-to-resume
gaps against the same clock; these include observer overhead and interrupts.
Callbacks buffer aggregate engine and gain diagnostics. Per-command RFPLL
tracing is disabled in this quiet observer to keep its cost out of the polls. All formatting and
UART, including failure reports, follow root return; the same observer is used
in both policy modes. A started PHY future is driven to completion.

Each hold root opens an image-only TIMG0 service timing window. The image reads
`oer-riscv::cycle64` on handler entry and around the public `Interrupt::service`
call, then measures an empty pair after the call. A fixed 4096-entry SPSC queue
retains four 64-bit values per event (128 KiB); overflow preserves earlier
samples and increments a saturating loss counter. The task drains it after root
return, never borrowing the handler cell. `pilot-service` notes report both
intervals minus that event's empty-pair cost, saturating unresolved intervals
at zero, and retain that cost. `pilot-window` reports events, losses, capacity
and CPU Hz. SOURCE: the staged S31 bootstrap selects `CpuProfile::Mhz160`;
the profile derives PLL/divider rates from the sourced HAL clock description.

Entry-to-call excludes the interrupt vector/prologue. Service duration includes
acknowledgement, custody transfer, the SAR machine advance, and rearm/reply; it
does not isolate the private SAR atom or terminal readiness. Those boundaries
are inside the production PHY and have no image observation API. Alarm-to-IRQ
cannot be read here: the IRQ owner holds only RAW/ST/CLR; the separate program
owner inside the PHY bundle holds counter/alarm registers. No production PHY
hooks or additional register access are introduced.

Measured on s31-a, run `1793149122405-001ccfd4`: corrected IRQ-entry-to-service median 0.244 µs; inclusive step-service median 17.5 µs / maximum 22.7 µs (observation only, no gate).

Stop retires the hold id and restores the saved policy; StopCapture then closes
RF. Overflow, reversed time, failed/missing/duplicate edges, incomplete branches,
cancellation or missing cleanup invalidate evidence. Parent and child intervals
are nested and cannot be summed. Grant-request duration is not proof of RF
exclusion. Associated Wi-Fi traffic is a separate workload.

`phy-tracking-hold-wifi-ordinary` selects `policy = Natural`: it preserves the
registered debug bytes and retained power/I2C state, checks actual committed
branches, and records 20 bounded batches of 16 ticks under one registration.
Each batch checks unchanged policy at stop; the final stop closes RF. This is
unassociated idle Wi-Fi, with no generated packet traffic. Per-batch engine
counters remain bounded independently of the number of batches.

`phy-tracking-idle-wifi-10m` and `phy-tracking-idle-ieee802154-10m` retain
one registration for 600 natural ticks, in forty batches of fifteen. They
record actual branch occurrences, bracket durations and engine maxima.
Natural-policy investigations retain long polls for diagnosis. Forced holds
check the engine's 1 ms continuous-poll guard. The ready-P2 response bound of
one atom, at most 50 microseconds, is qualified separately with scheduler,
timer and interrupt proofs; aggregate engine maxima alone do not establish
that response. Policy checks and RF cleanup apply to both modes. Batch
handshakes add time to the 600 one-second tick intervals.

Registration and bring-up checkpoints use the console's reliable text queue;
the console owns USB writes for both frames and checkpoint lines. Reporting
runs after the native registration result and outside its measured duration.

The hardware-free hold recorder, metrics, note sink contract and SAR readiness
distribution live in `oer-esp32s31-phy-probe::hold`, with their own host tests.
This image retains the concrete PHY observer, clock/poll wrapper, queued console
and calibration-cache adapter as composition. Host tests of the image exercise
observer output after root return and cache admission/export; vendor workloads
include no image sources.
