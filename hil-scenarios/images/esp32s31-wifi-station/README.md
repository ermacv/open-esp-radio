# ESP32-S31 Wi-Fi station image

The image the HIL Wi-Fi station scenarios of the ESP32-S31 share. It starts
the shared staged platform, serves the HIL base module over the USB
Serial/JTAG console and mounts two services:

| Service | Handler |
| --- | --- |
| [Wi-Fi station](../../services/wifi-station/src/lib.rs) | The product station facade (`oer-esp32s31-wifi`) with IPv4 by DHCP (`oer_embassy_net::ip`); one request at a time, a request while one waits is refused as `Busy` |
| [Load](../../services/load/src/lib.rs) | UDP load on the station's address and the executors' CPU (`src/load`); one request at a time, likewise |

## Composition

The image runs both harts (`harts = ["boot", 1]`): the radio on the boot
hart, the network on hart 1, across one owned-packet boundary, the
station's network end.

- **Hart 0.** The boot composes the shared RF actor with its SAR/PWDET
  binding, the radio executor (P2: the actor and the Wi-Fi driver), the
  Wi-Fi interrupt (P3) and its doorbell, and keeps every owner the
  station's construction takes. The application executor (thread mode,
  `FROM_CPU0`) runs the console, the station's work (its requests, its
  control and its runner), the relay of the load server (`src/load/relay.rs`)
  and the RTC watchdog's feed. Lines: P4 PC sampler (TIMG1 T0) 22; P3 Wi-Fi
  handler 21 and radio alarm (SYSTIMER comparator 0) 19; P2 radio executor
  17 and TIMG0 SAR/PWDET steps 16; P1 executor wake 40, alarm (comparator 1)
  41 and console 42.
- **Hart 1.** During boot, before any executor, the boot hart installs the
  time driver (the radio executor writes the SYSTIMER), grants hart 1's
  executor comparator 2 and launches hart 1 with the root's permit
  (`oer_esp32s31_compose::secondary::launch_hart1`). Hart 1 makes its own
  interrupt context, routes its alarm and starts its interrupt-mode
  executor with one task, `network` (`src/network`): it waits for the
  station's network end, starts the IP stack over it, runs the IP service,
  the station's network stop and the load server with its sockets. Lines of
  hart 1: P2 network executor (`FROM_CPU1`) 16, P3 its alarm (comparator 2)
  17. Hart 1 takes no critical section and writes no SYSTIMER register but
  its alarm's acknowledgement.

No waker crosses harts. Every cross-hart wake raises the receiving task's
executor signal (`FromPeerHart`, `src/peers.rs`), and every queue end on a
hart notifies through its task's signal:

| Signal | Task | Raised by | Ends that wait on it |
| --- | --- | --- | --- |
| network | hart 1's `network` | hart 0's application tasks | the station's receive and copies, transmit room, link and stop request; the start; the exit; the load requests; the relay's replies and command room |
| station | hart 0's `station_work` | hart 1's `network` | the station's transmit, receive and copies room, the network stop's answer and the lent buffers' return; the start's answer; the exit's end; the lease |
| relay | hart 0's `relay` | hart 1's `network` | the relay's commands and reply room |
| driver | hart 0's radio `driver` | hart 1's `network` (and the station work below it) | the driver's request for its lent buffers |

A lent receive buffer the station itself drops (a delivery its network
queue refused) raises the station's signal from the station work's own
task; the executor's next round wakes it, a raise the signal's declaration
does not name.

The boot takes one join. `Join` validates the SSID, the passphrase and the
channel, derives the PMK, then constructs the station for that channel,
starts its driver on the radio executor, hands the station's network end
with the station's address and a seed to hart 1, waits until hart 1 started
the IP stack over it, and connects. The station's status reads the DHCP
lease hart 1 publishes (one writer, read whole under a sequence number);
the station's exit asks hart 1 to stop the IP service and waits for its
end. `Joined::Linked` answers once the connect completed: associated
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
boundaries, the image's named parts their poll boundaries, the station
service its spans and the network bridge its frames, and the station
configuration names no access point to observe. The driver's observer is the
image's, which counts the spans and the bridge's frames; the station's stays
`Noop`.

`Start` binds one UDP socket on its port of the station's DHCP address and
answers `Running` at once; without a lease it is refused (`NoAddress`), and
while a run runs (`Busy`). Its parameters are refused (`InvalidParameters`)
for a zero port or peer port, a delay, window or drain past the service's
maximum, or a payload below the 28-byte load header or above 1472 bytes,
the UDP payload of one 1514-byte Ethernet frame (the stack fragments
nothing). The run's window opens `delay_ms` after the start and closes
`duration_ms` later; the image reads its CPU, its parts and its counters at
both ends, after opening and after closing, and keeps the times. It receives
and echoes from the start until `drain_ms` after the window, then closes the
socket and keeps the report, the parts', the counters', the link's and the
PHY tracking's window until the next run starts. `Collect`, `CollectParts`
(twelve parts an answer, from the asked number), `CollectCounters`,
`CollectLink`, `CollectTracking` and `CollectSamples` (forty samples an
answer, from the asked number) of the running run answer `Running`, of the
kept one what it kept, of any other `UnknownRun`; `CollectSamples` of a run
without a profile answers `NotSampled`.

The load server runs on hart 1 beside the IP stack; the console, the PC
sampler and the counters it reads are the boot hart's. It reaches them
through the boot hart's relay task (`src/load/relay.rs`): one queue of
commands (publish an answer, publish a samples page, the sampler's
availability, start, stop request and stop, the boot hart's counters) and
two reply queues, each across harts. A call numbers its command and takes
only its own reply, so a call the server abandoned (a station stop during
a window) leaves a reply the next call discards. An answer goes out once
the relay's task runs on the boot hart: after the server queued it, not
when it decided it.

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
  room for is dropped (`refused`) and its time passes; nothing is sent
  after the window ends, and what was due but not yet sent then is
  `missed`; other errors count
  as `failed`. At rate 0 it sends as fast as the socket takes datagrams,
  yielding every 16.
- **CPU.** Each executor's task set selects the observer of its own slot
  (the boot hart's application and radio executors, hart 1's network
  executor), which accumulates the executor's work spans and, for the boot
  hart's thread executor, its idle spans (masked WFI), in 64-bit totals.
  `GetCpu` and each window end read them since boot with the image's time.
  Work spans include preemption and overlap between executors, so they
  never add up to busy time; only a hart's idle time gives its utilisation
  (`1 - idle / window` between two reports), one row per hart. Hart 1 runs
  only its network executor, polled in its interrupt, while its main waits
  for interrupts: its idle time is the report's time less that executor's
  work (its alarm handler outside a round counts as idle). A build without
  `oer_trace` reports no harts or executors.
- **Parts** (`src/load/parts.rs`). The tasks (`console`, `station-work`,
  `rtc-watchdog`, `relay` on the boot hart's thread executor;
  `radio-actor`, `driver` on its P2; `network` on hart 1), the station
  work's futures (`station`, `requests`) and the network's (`ip`, `load`)
  are each polled through `Polled`, which observes the start and end of
  every poll through `oer_trace::observe!`. Each part counts its polls,
  their time and its longest poll; the window's start begins a new epoch
  for the longest. A poll's time includes its preemption, so a task's
  futures nest inside its task and the boot hart's P2 parts preempt its
  thread parts: shares do not add up. Inside `station` the service's spans
  (`step`, `pending`, `sources`, `received`, `deliver`, `queue`,
  `terminal`, `transmit`, `recycle`) count the same way, reported through
  the driver's observer. A build without `oer_trace` keeps no part.
- **Counters** (`src/load/counters.rs`). At both window ends the image reads
  the boot hart's 64-bit cycle and retired-instruction counters (the radio
  and the station: all its work and interrupts included), which its relay
  reads there, the frames the IP stack took from its device, and what the
  network bridge did with the station's frames: taken, or refused for want
  of a pool packet, of queue room or for their length or a down link. The
  bridge reports each frame through `oer_trace::observe!`; a build without
  `oer_trace` counts none of them.
  Just before the window's start the relay turns the L1 cache's trace on,
  then clears and enables the L1 cache's access counters of instruction
  buses 0 and 1 and data buses 0 and 1 (`Hardware::cache_counters`,
  `oer_esp32s31_hal::peripherals::cache_counters`, owned by the relay on
  the boot hart), and reads each bus's hits, misses, conflicts, next-level
  reads (a data bus also its next-level writes) and overflow bit at both
  ends, with the configured cache geometry at the end.
  The receive path's counts come from the same relay readings on the boot
  hart: the S31 driver's receive counters, the radio's, which a trace build
  of the Wi-Fi facade lends (`oer_esp32s31_wifi::rx_counters`), and the
  station service's receive decisions, which the facade's
  `TracedStationEvents` sink reports as `StationRx` events to the image's
  observer in the station service on the boot hart, where they count.
  Without `oer_trace` that sink is disabled and the service builds no event.
  The link's counts (`CollectLink`) come from the same readings: the
  facade reports each data MPDU addressed to the station's own address with
  its PPDU's rate and A-MPDU containment from the receive metadata
  (`RxData`, from the port's event hook), and each single data MPDU
  transmission's end with its last transmission's rate and its
  transmission count (`TxData`, from the rate selection's completion), to
  the image's observer, which the binding also gives the rate selection.
  They count by HT MCS 0 to 7, width and guard interval, other HT MCSs,
  non-HT, HE and no rate. The receive metadata marks no A-MPDU boundary, so
  the image counts no received A-MPDU; transmitted aggregates are not
  counted yet. A build without `oer_trace` counts none of them.
- **PHY tracking** (`src/load/tracking.rs`, `CollectTracking`). The shared
  RF actor reports each periodic tracking tick (`TrackingTick`: tracked,
  not due or unavailable, the branches a tracked tick committed, the PHY's
  temperature after it in sensor units and the tick's duration) to the
  image's observer in the radio executor (P2) on the boot hart. The load
  server samples the counter on hart 1 at the window's ends, as it samples
  the parts: a sequence number brackets each write and a sample taken
  during one is taken again. The last, minimum and maximum temperature and
  the longest tick belong to the parts' epoch, which starts at the
  window's start. A build without `oer_trace` reports `available: false`.
- **Samples** (`src/load/sampler.rs`). A run whose `Start` has a `profile`
  starts `oer_esp32s31_compose::sampler` on TIMG1 T0 of the boot hart,
  which it samples, at the window's start and asks it to stop at the
  window's end, before the end's readings, both through the relay. Its
  P4 handler reports each scheduled tick's interrupted `mepc` and
  `mcause.mpil` (0 for task code), and the interrupted `ra` its interrupt
  entry recorded (`oer_espressif_staged_runtime::stacks::interrupted_ra`),
  through the image's observer into one `oer_trace::profile::Thinning` of
  2048 samples (24 KiB of SRAM), which is normalized to one stride once the
  session is back; each page sample carries its `ra`. The load server waits
  at most four periods and 10 ms for it; a handler that keeps it leaves the
  sampler gone for the image's run (`NoSampler`), as does a build without
  `oer_trace`. A disarmed sampler's timer is stopped and masked.

The IP stack's pools are sized for about 65 Mbit/s of 1400-byte datagrams
each way (`src/network/ip.rs`): 32 receive slots for the station's bursts,
16 transmit credits, 32 general slots, and 16 datagrams queued per socket.
The stack's storage is written from a constant image, never built on the
task stack. The load runs on hart 1 beside the IP stack, in hart 1's one
task.

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
the previous boot's PHY, Wi-Fi MAC and radio tracking fault records. A
failed boot check of either hart, a construction error the facade cannot
return, an exhausted nonce stream or an IP stack failure is retained and
resets (`src/fatal.rs`); a failed periodic PHY tracking tick, or a tracking
deadline the clock cannot hold, ends the shared RF actor's run with its
keeping every owner, and the actor's task records the fault's class and
variant path (`oer_esp32s31_compose::radio_fault`) and resets; a failure of
hart 1's launch after its release records its stage and resets (`previous
fault SecondaryLaunch` or `SecondaryEarlyFault`). An exception keeps the
runtime's trap-time frame and the platform's observations after the trap
(`oer_esp32s31_compose::exception`), both printed by the next boot; the
console's transport faults reset through the platform's retained path.

`cargo fw build esp32s31-wifi-station` builds it; a scenario's harness builds
and flashes it.
