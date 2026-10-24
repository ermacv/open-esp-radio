# ESP32-S31 HIL target

- [System and PHY watchdogs](watchdogs.md)
- [Stack measurements](memory.md)
- [Host setup and fixture installation](../../../tools/hil/run/HOST.md)
- [Wire and evidence contracts](../../../protocols/hil/protocol/README.md)

This workspace selects the shared board boot and memory profile and owns the
Embassy executor, the USB Serial/JTAG transport and the HIL workloads of the
system images and the personal station image. Effective dependency locks are
archived beside each image.

## Application boundaries

The agent binary is the HIL application of the shared stage-two boot
contract. Each image class selects one base feature: `system-watchdog`,
`system-panic-reset`, `boot-smoke` or `wifi-station` (`usb-jtag-off` adds to
`system-watchdog`). Board initialization, relocation and interrupt-stack
mechanics belong to the shared platform; an application image would use the
same mechanism through `oer-image-build`. A hardware scenario verdict remains a
separate HIL responsibility.

## Protocol families per image

Each image enables only the HIL protocol families it serves
([radio families](../../../protocols/hil/protocol/README.md#radio-families)): the radio-free
`system-watchdog`, `diagnostic-usb-jtag-off` and `system-panic-reset` images
read only the shared core, `system` module included.

## Personal station

`wifi-station` serves `wifi/station/start`, correlated link events and the
network-ready event on the USB console. One `StartStation` provisions a boot;
another configuration in that boot is rejected with `InvalidState`. Credentials
arrive only in the binary HIL command. P1 derives the PMK with 8192 bounded
PBKDF2-HMAC-SHA1 steps, yielding between them, and retains a zeroizing
`SaePassword` for the personal station. The received passphrase is erased before
the first yield. Cancelling derivation drops its zeroizing password and partial
key owner. The `station-pmk` note reports only elapsed microseconds.

The image installs the lab's DE regulatory domain with a 10 dBm owner cap and
scans permitted HT20 channels 1–13 for the supplied SSID. It uses the existing
placed StationService, arbiter and owned Embassy/Xarxa DHCP/ICMP stack on P1;
the radio actor, hardware backend and DMA owners run on P2. A single LP TRNG
source owner supplies a fresh 256-bit ChaCha20 seed before moving to P2. The
P1 SNonce stream continues across reconnects. Stream or epoch exhaustion stops
through the reset path; it never substitutes a nonce or wraps an epoch.

Successful Association processing emits `Associated`; the controlled-port
notice emits `Authorized`. The controller retains the Connect credit returned by its terminal; a
disconnect enables another attempt with the next epoch. A poisoned or
quarantined radio ends the service through the existing fail-stop path; the
controller cannot obtain a reusable radio owner. The backend grants one group
slot, retaining its fail-stop GTK rollback policy.

The W4 host controls 100-echo and ten-echo windows through `ProbeStation` and
ends the run with `StopStation`. The image reuses W3's checked ICMP codec and
distinct-pair observer. Typed observations expose hardware GTK installation
KeyIDs, decoded broadcast ARPs, timeouts/collisions and final deauthentication;
the host computes acceptance. Only a clean service stop emits `StationStopped`;
a service that ends otherwise emits one `StationServiceEnded` with its typed
fault, and the console then refuses `StopStation` and `ProbeStation` with
`InvalidState`. Disconnect reasons and faults map to wire values by exhaustive
matches (`wifi_station/report.rs`).

The personal image's `stack-wifi-station.toml` names the measured P1
frames that exceed the move limit, each with its own checked ceiling. IRQ
stacks retain the normal per-hart guard and percentage margin and are sized
from the actual P2/ISR bound. Polling and destruction of placed worker futures
stay outside the small Embassy task shells.

## Panic and exception records

The agent's panic hook, which the platform's panic entry calls through its
`panic-hook` feature, runs in every image but `system-panic-reset` (the
agent's `panic-hook` feature, which each image's base feature enables); that
image runs the product panic entry alone, so the `system-panic-reset`
scenario exercises the product's record-and-reset path. The hook and the
exception entry format nothing: they record the post-mortem fault and the
hart's trap registers, the vector it took and the sources pending and routed
to it (`agent/src/fatal.rs`) in RTC fast memory and reset, and the next boot
prints `OPEN_RADIO_HIL runtime=PANIC boot=previous` or `runtime=EXCEPTION
boot=previous` with that state before anything else, as typed notes
(`OER-DIAG:01` lines through the ROM) that the host renders into those lines. A panic message with
arguments is not kept, only a static one in task context. An
interrupt-context panic omits location and message because compiler metadata
may be in unavailable PSRAM; the hook receives the platform's safe
`PanicDetails`. The hook's code and diagnostic stage stay in SRAM, its
records stay in RTC memory, and its fault CRC uses no cached lookup table.

