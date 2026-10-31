# ESP32-S31 Wi-Fi station image

The image the HIL Wi-Fi station scenarios of the ESP32-S31 share. It starts
the shared staged platform, serves the HIL base module over the USB
Serial/JTAG console and mounts two services:

| Service | Handler |
| --- | --- |
| [Wi-Fi station](../../services/wifi-station/src/lib.rs) | The product station facade (`oer-esp32s31-wifi`) with IPv4 by DHCP (`oer_embassy_net::ip`); one request at a time, a request while one waits is refused as `Busy` |
| [Load](../../services/load/src/lib.rs) | UDP load on the station's address and the executors' CPU (`src/load`); one request at a time, likewise |

## Composition

The boot composes the shared RF actor with its SAR/PWDET binding, the radio
executor (P2), the Wi-Fi interrupt (P3) and its doorbell, and keeps every
owner the station's construction takes; the application executor (P1) runs
the console, the station's work with the load server beside it in one task,
and the RTC watchdog's feed. Hart 0 lines:
P3 Wi-Fi handler 21 and radio alarm 19; P2 radio executor 17 and TIMG0
SAR/PWDET steps 16; P1 executor wake 40, alarm 41 and console 42
(`docs/architecture.md`, Execution).

The boot takes one join. `Join` validates the SSID, the passphrase and the
channel, derives the PMK, then constructs the station for that channel,
starts its driver on the radio executor and the IP stack beside it, and
connects. `Joined::Linked` answers once the connect completed: associated
and authorized. `GetStatus` answers in every phase with the station's own
address, the access point it is linked to and the DHCP lease. `Leave` stops
the station gracefully (disconnect, link down, the station's runner retired)
and answers `Left::Down`; the IP stack stops with it. A second join, or a
request after the station stopped, is refused (`AlreadyJoined`,
`Unavailable`). The IP stack answers ICMP echo once it holds a lease.

The image builds in no credentials. It does not radiate before `Join`: the
RF actor calibrates and the radio starts at the station's first
registration, which the join's station makes.

## Load

The image declares the `trace` instrumentation class: `cargo fw` builds it
with `oer_trace`, under which the executors report their activity
boundaries, and the station configuration names no access point to observe.
Every other observer of the station stays `Noop`.

`Start` binds one UDP socket on its port of the station's DHCP address and
answers `Running` at once; without a lease it is refused (`NoAddress`), and
while a run runs (`Busy`). Its parameters are refused (`InvalidParameters`)
for a zero port or peer port, a delay, window or drain past the service's
maximum, or a payload below the 28-byte load header or above 1472 bytes,
the UDP payload of one 1514-byte Ethernet frame (the stack fragments
nothing). The run's window opens `delay_ms` after the start and closes
`duration_ms` later; the image reads its CPU at both ends, after opening
and after closing, and keeps the times. It receives and echoes from the
start until `drain_ms` after the window, then closes the socket and keeps
the report until the next run starts. `Collect` of the running run answers
`Running`, of the kept one its report, of any other `UnknownRun`.

- **Receive.** A datagram whose header asks for an echo, of any run, goes
  back to its source at once with the reply flag instead (`echoed`, or
  `echo_refused` when the socket has no room) and is never load. Every
  other datagram with the run's header is load, counted by the service's
  shared tally of 8192 words (64 KiB, sequence numbers below 524 288, some
  90 s at 65 Mbit/s of 1400-byte datagrams): distinct, late, duplicate,
  lost or, past its capacity, untracked. A distinct one that arrives within
  the window also counts as the window's. Anything else on the port is
  `foreign`.
- **Transmit.** Within the window, datagrams of `payload` bytes (header
  and zero filler) go to the peer from the bound port, with sequence
  numbers from 0 counting only the datagrams the socket took. At a nonzero
  rate datagram `n` is due `n` payload times after the window opened: the
  sender wakes on the Embassy timer, sends what is due, at most 16 a batch,
  and sleeps at least 1 ms between batches. A datagram the socket has no
  room for is dropped (`refused`) and its time passes; other errors count
  as `failed`. At rate 0 it sends as fast as the socket takes datagrams,
  yielding every 16.
- **CPU.** Each executor's task set selects the observer of its own slot,
  which accumulates the executor's work spans and, for the hart's thread
  executor, its idle spans (masked WFI), in 64-bit totals. `GetCpu` and
  each window end read them since boot with the image's time. Work spans
  include preemption and overlap between executors, so they never add up
  to busy time; only a hart's idle time gives its utilisation
  (`1 - idle / window` between two reports). A build without `oer_trace`
  reports no harts or executors.

The IP stack's pools are sized for about 65 Mbit/s of 1400-byte datagrams
each way (`src/station/ip.rs`): 32 receive slots for the station's bursts,
16 transmit credits, 32 general slots, and 16 datagrams queued per socket.
The stack's storage is written from a constant image, never built on the
task stack. The load runs on P1 beside the station and its IP stack.

## Regulatory configuration

Country `DE` (`oer-radio-regulatory`), the 2.4 GHz channel the join names
with 20 MHz width and an owner transmit cap of 10 dBm below the domain's
ceiling. The station scans only that channel; automatic reconnect is
disabled.

## Failure codes

`StationError::Failed { code }` is four bytes, most significant first: the
class, a kind and a 16-bit detail
([`runtime`](../../services/wifi-station/src/runtime.rs) numbers classes 1
to 3, the image classes 4 to 6).

| Class | Meaning | Kind | Detail |
| --- | --- | --- | --- |
| 1 | The connect ended without a link | The `DisconnectReason`: 0 local request, 1 peer, 2 authentication failed, 3 SAE, 4 association failed, 5 not found, 6 handshake failed, 7 SA Query timeout, 8 transmit refused, 9 channel refused, 10 lease exchange refused, 11 arbiter, 12 deadline overflow, 13 encode, 14 no admitted access category, 15 invalid EAPOL, 16 retained security IEs | The peer's reason code (1), the SAE peer reason or status code (3), 1 for a partial scan (5); otherwise 0 |
| 2 | The station refused the connect | 0 already connected, 1 not connected, 2 transitioning, 3 a terminal of another command | 0 |
| 3 | The station's control did not admit the connect | 0 queue full, 1 stopped | 0 |
| 4 | The station ended before the join linked or the leave stopped it | 0 stopped, 1 failed, 2 poisoned, 3 quarantined, 4 abandoned, 5 construction refused | For 5, the `InitializationError`: 0 PHY, 1 country, 2 domain, 3 channel, 4 address, 5 SSID, 6 arbiter, 7 storage, 8 control, 9 frontend, 10 membership unavailable, 11 request, 12 radio, 13 backend, 14 registration, 15 RX, 16 TX, 17 DMA |
| 5 | The IP stack refused to start | The `oer_embassy_net::ip::Error`: 0 invalid prefix length, 1 endpoint already split, 2 RX waiter unavailable, 3 interface table full, 4 unsupported medium, 5 hardware address mismatch, 6 address not unicast, 7 address table full, 8 gateway not unicast, 9 route table full, 10 DHCP medium mismatch, 11 context access refused | 0 |
| 6 | The credentials could not be prepared | 0 SSID length, 1 passphrase length | 0 |

## Faults

The boot reads the reset causes and peeks the previous boot's platform panic
before serving boot evidence; complete output clears the record. It prints
the previous boot's PHY and Wi-Fi MAC fault records. A failed boot check, an
exception, a construction error the facade cannot return, an exhausted nonce
stream or an IP stack failure is retained and resets (`src/fatal.rs`); the
console's transport faults reset through the platform's retained path.

`cargo fw build esp32s31-wifi-station` builds it; a scenario's harness builds
and flashes it.
