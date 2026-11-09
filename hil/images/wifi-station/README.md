# ESP32-S31 station image

The image of the [SDK station](../../scenarios/sdk-station/README.md),
[Wi-Fi station](../../scenarios/wifi-station/README.md) and
[Wi-Fi load](../../scenarios/wifi-load/README.md) scenarios: an
application of the entry `oer` (`oer`, features
`wifi`, `console`, `group`, `observe` and `network`; the build selects the chip). Its boot takes the Wi-Fi
station (on the AES accelerator) with the library's defaults and the USB
Serial/JTAG console; its group starts the network core
(`wifi::Network::start`, protocols/network) over the station's device in
place of the IP service. The library places their interrupts, executors and
service tasks and records and resets on the image's behalf. The image
declares no interrupt, DMA or service task of its own. Its boot is the
[station example's](../../../examples/wifi-station/README.md) with
these differences: the `observe` feature with the `trace` and
`fault-injection` instrumentation (the library's observations, which the
load service reports, and the fault service's refused publication), the
`network` feature (the network core in the IP service's place: ARP, IPv4,
UDP, a DHCPv4 client and ICMP echo, without TCP), the `console` feature and
`Console::open`, and every channel scanned (`wifi::Config::all_channels`)
in place of the profile's one. Its group
(`entry!(boot, group = network)`) starts the network with the load's
channel socket and producer bound at the start, answers the worker's asks
for the network's address and its waits, and runs the load server on the
network's sockets; the worker reaches them over a `Channel` each, and the
load server reaches the boot hart's counters and sampler over another. The
fault service is a handler, not a change to the boot. Review keeps the two
boots in step; `oer-tools-test-layers` checks only that the image takes
nothing the SDK places.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../agent/README.md); the boot evidence is the library's (`Boot::evidence`): hart 0's reset cause, the previous boot's panic, and its CPU exception as the post-mortem (`Fault::Exception`), which the library already printed and cleared |
| [SDK station](../../services/sdk-station/src/lib.rs) | One worker on the application executor answers the requests in order; a request while one waits is refused as `Busy` |
| [Wi-Fi station](../../services/wifi-station/src/lib.rs) | The same worker, after the service's phases (`oer_hil_services_wifi_station::phase`): one join per boot, a leave of the joined station once |
| [Fault](../../services/fault/src/lib.rs) | The same worker answers `RaiseException`, waits until the console wrote the answer and traps on an illegal instruction (`answer_then_trap`); a write the console cannot confirm ends the application with `sdk-console`, recorded and reset, without the trap. `RefusePublication` has the station refuse its next publication (`oer_fault_injection`), and `GetRestarts` answers the station's restarts without a reset and the last one's cause |
| [Load](../../services/load/src/lib.rs) | The same worker hands each request to the load server in the group and publishes its answer (`src/load`) |

The application is the agent's `serve`: the console and the worker side by
side. The boot identity is the library's (`oer::boot_id`).

The group answers the worker's asks one at a time, so at most one wait
reads the network's publications (`src/waits.rs`). Each publication holds
the interface's address, the DHCP client's state and the link as the core
saw it, so a wait reads the address and the link of one moment. A read the
watch refuses is an impossible state on the group's hart, where the service
publishes within its own turn: it ends the group, which records it.

- **Connect** builds a WPA2-Personal profile and awaits the station's
  `connect`: `Linked`, or `Failed` with the code the
  [Wi-Fi station service](../../services/wifi-station/src/runtime.rs)
  numbers in its classes 1 to 3 (the disconnect reason, a refusal, a queue
  that did not admit it). Once linked, the station keeps the network and
  rejoins it every second after a lost association; a connect without a
  link leaves it idle for the next one.
- **WaitIpv4** waits, bounded by the request's timeout, for an address
  over an up link while the DHCP client holds it bound: a lease a server
  granted, confirmed or extended. A client still selecting, requesting or
  rebooting holds none yet: after a link loss
  the client's INIT-REBOOT keeps the old address in use but unconfirmed
  until a server's ACK, so the wait goes on to that ACK or the next lease.
  `Stopped` once the network ended.
- **WaitLinkDown** waits, bounded by the request's timeout, until the link
  up at its start is down or up in a later epoch, as the network saw it; a
  link down at the start ends it at once.
- **Exchange** is refused as `NoTcp`: the network core has no TCP, and the
  image opens no connection.
- **GetStatus** answers the station's MAC address, whether its link is up
  and the network's address.

The Wi-Fi station service's requests go to the same station:

- **Join** builds a WPA2-Personal profile, refuses a channel the domain does
  not permit as `InvalidSettings` (the image stays idle for another join)
  and awaits the station's `connect`. The station scans every permitted
  channel, so the join's channel is the host's check of the link, not a
  setting: `Linked` names the channel the station found. A failure answers
  `Refused` with the same codes as the SDK station service's `Connect`.
- **GetStatus** answers the station's address, the access point and channel
  while the link is up (`Station::linked`) and the network's address.
- **Leave** stops the station (`Station::stop`): the network returns its
  receive buffers within the default drain of two seconds, the radio
  retires, and the answer is `Left::Down`. Every later request of either
  station service finds the station stopped.

## Load

The load server runs in the group on hart 1, beside the network;
the boot hart's relay, a part of the application beside the worker, serves
what it needs there: the boot hart's cycle, instruction and cache counters
(`Boot::observe`) with the library's counts (`observe::counters`), read
together, the PC sampler, and a page of samples, which the relay answers
itself. The server sends one command at a time and waits for its reply.
Hart 1's PC sampler (`GroupBoot::observe`) is the server's own.

- **Start** takes the network's channel socket, bound at the group's start
  on port 5001 of the station's DHCP address, and answers `Running` at once;
  the run's datagrams leave through the producer, bound on 5002 (`src/udp.rs`).
  Without an address it is refused (`NoAddress`), while a run runs (`Busy`).
  Its parameters are refused (`InvalidParameters`) for another port than
  5001, a zero peer port, a delay, window or
  drain past the service's maximum, a payload below the 28-byte load
  header or above 1472 bytes (one 1514-byte Ethernet frame: the stack
  fragments nothing), or a sampling period outside 250 to 10000 µs. The
  window opens `delay_ms` after the start and closes `duration_ms` later;
  the server reads the CPU, the parts, the counters, the link and the PHY
  tracking at both ends and keeps the window's differences, with the
  times, until the next run starts. It receives and echoes until
  `drain_ms` after the window, then closes the socket.
- **Collect…** of the running run answer `Running`, of the kept one what
  it kept, of any other run `UnknownRun`; a run without a profile answers
  `NotSampled` to `CollectSamples`.
- **Receive.** A datagram whose header asks for an echo goes back at once
  with the reply flag and is no load; every other datagram of the run
  counts in the service's tally of 8192 words (sequence numbers below
  524 288), anything else is `foreign`.
- **Transmit.** Within the window, datagrams of `payload` bytes go to the
  peer, paced at a nonzero rate (what is due, at most 16 a batch, at least
  1 ms between batches; a full socket's datagram is `refused`, what was
  due at the window's end `missed`), or as fast as the socket takes them at
  rate 0.
- **CPU** (`GetCpu` and the window's ends): each executor's work and idle
  since boot (`observe::executors`): one hart row per hart with idle time,
  one executor row per executor with work.
- **Parts:** the library's (`application`, `watchdog`, `station`,
  `datapath`, `radio-actor`, `ip`, `group` and the datapath actor's spans
  `turn`, `transmit`, `receive`, `intake`, `messages`) and the
  image's two, `relay` on the boot hart and `load` on hart 1, twelve an
  answer.
- **Counters, link, tracking:** hart 1's cycles and retired instructions
  at the window's two ends, read on hart 1, and the window's differences of
  the boot hart's counters, the bridge's frames, the datagrams the network delivered, the network core's turns and
  their profile (`CollectProfile`, three pages: the fit's sums, the cells,
  the histograms with the station's return bursts), the
  cache's counts (cleared just before the window) and geometry, the
  receive driver's and the station's receive counts, the transmit
  agreements', the link's rates and the PHY tracking ticks.
- **Samples:** a run whose `Start` has a `profile` samples both harts
  every period over the window (`observe::Sampler`, 1024 samples each), 27
  an answer, by hart (`CollectSamples::hart`); a boot-hart sampler a
  session kept refuses the next sampled run (`NoSampler`), and hart 1's
  refusal leaves only hart 1 unsampled.

A stop of the station closes the run's socket with the network (its
queued datagrams return their lent buffers); the run then receives and
sends nothing more and ends at its time.

## Regulatory configuration

Country `DE` (`oer-protocols-radio-regulatory`): the station scans every 2.4 GHz
channel the domain permits at each connect (`wifi::Config::all_channels`),
joins at 40 MHz where the access point offers a permitted secondary channel
(`with_ht40`), otherwise at 20 MHz, at the transmit power the domain permits: the image's
cap of 20 dBm stays above that ceiling.

## Faults

A boot error, an application error and every unrecoverable station exit
record their named cause and reset the chip (the library's failure table);
a console transport fault ends the application with `sdk-console`. The
image builds in no credentials.

`cargo fw build hil-wifi-station` builds it; the scenario's harness
builds and flashes it.
