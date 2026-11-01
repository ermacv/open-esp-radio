# ESP32-S31 SDK station image

The image of the [SDK station](../../sdk-station/README.md),
[Wi-Fi station](../../wifi-station/README.md) and
[Wi-Fi load](../../wifi-load/README.md) scenarios: an
application of the entry `oer-espressif` (`sdk/espressif`, features
`esp32s31`, `wifi`, `console`, `hart1`, `observe` and `udp`). Its boot takes the AES service, the Wi-Fi
station and its IP service with the library's defaults and the USB
Serial/JTAG console; the library places their interrupts, executors and
service tasks and records and resets on the image's behalf. The image
declares no interrupt, DMA or service task of its own. Its boot is the
[station example's](../../../firmware/apps/wifi-station/README.md) with
these differences: the `observe` feature with the `trace` instrumentation
(the library's observations, which the load service reports), the `udp`
feature (the load's socket), the `console` feature and `Console::open`, every
channel scanned (`wifi::Config::all_channels`) in place of the profile's
one, and its own IP pools, 32 receive, 16 transmit and 32 general frames
(`Net::start_with` with a static `wifi::Pools` in the second boot), in
place of the library's 8, 8 and 8. Like the example, its second boot
(`entry!(boot, hart1 = network)`) starts the IP service and runs the line
exchanges and the load server on its sockets; the worker reaches them over
a `Channel` each, and the load server reaches the boot hart's counters and
sampler over a third. The fault service is a handler, not a change to the boot. Review keeps the two boots in step; `oer-test-layers` checks only that
the image takes nothing the SDK places.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../hil-agent/espressif/README.md); the boot evidence is the library's (`Boot::evidence`): hart 0's reset cause, the previous boot's panic, and its CPU exception as the post-mortem (`Fault::Exception`), which the library already printed and cleared |
| [SDK station](../../services/sdk-station/src/lib.rs) | One worker on the application executor answers the requests in order; a request while one waits is refused as `Busy` |
| [Wi-Fi station](../../services/wifi-station/src/lib.rs) | The same worker, after the service's phases (`oer_services_wifi_station::phase`): one join per boot, a leave of the joined station once |
| [Fault](../../services/fault/src/lib.rs) | The same worker answers `RaiseException`, waits until the console wrote the answer and traps on an illegal instruction (`answer_then_trap`); a write the console cannot confirm ends the application with `sdk-console`, recorded and reset, without the trap |
| [Load](../../services/load/src/lib.rs) | The same worker hands each request to the load server in the second boot and publishes its answer (`src/load`) |

The application is the agent's `serve`: the console and the worker side by
side. The boot identity is the library's (`oer_espressif::boot_id`).

- **Connect** builds a WPA2-Personal profile and awaits the station's
  `connect`: `Linked`, or `Failed` with the code the
  [Wi-Fi station service](../../services/wifi-station/src/runtime.rs)
  numbers in its classes 1 to 3 (the disconnect reason, a refusal, a queue
  that did not admit it). Once linked, the station keeps the network and
  rejoins it every second after a lost association; a connect without a
  link leaves it idle for the next one.
- **WaitIpv4** and **WaitLinkDown** bound the IP service's waits by the
  request's timeout.
- **Exchange** goes to the second boot's future over the channel, which
  opens one TCP socket (128-byte buffers) on the IP stack, sends the line,
  reads as many bytes back and closes, within the request's timeout;
  without an address it answers `NoAddress`. The worker waits for its
  answer.
- **GetStatus** answers the station's MAC address, whether its link is up
  and the IPv4 configuration over it.

The Wi-Fi station service's requests go to the same station:

- **Join** builds a WPA2-Personal profile, refuses a channel the domain does
  not permit as `InvalidSettings` (the image stays idle for another join)
  and awaits the station's `connect`. The station scans every permitted
  channel, so the join's channel is the host's check of the link, not a
  setting: `Linked` names the channel the station found. A failure answers
  `Refused` with the same codes as the SDK station service's `Connect`.
- **GetStatus** answers the station's address, the access point and channel
  while the link is up (`Station::linked`) and the IPv4 configuration.
- **Leave** stops the station (`Station::stop`): the IP service returns its
  receive buffers within the default drain of two seconds, the radio
  retires, and the answer is `Left::Down`. Every later request of either
  station service finds the station stopped.

## Load

The load server runs in the second boot on hart 1, beside the IP stack;
the boot hart's relay, a part of the application beside the worker, serves
what it needs there: the boot hart's cycle, instruction and cache counters
(`Boot::observe`) with the library's counts (`observe::counters`), read
together, the PC sampler, and a page of samples, which the relay answers
itself. The server sends one command at a time and waits for its reply.

- **Start** binds one UDP socket on its port of the station's DHCP address
  and answers `Running` at once; without an address it is refused
  (`NoAddress`), while a run runs (`Busy`). Its parameters are refused
  (`InvalidParameters`) for a zero port or peer port, a delay, window or
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
- **Parts:** the library's (`application`, `watchdog`, `aes`, `station`,
  `radio-actor`, `driver`, `ip`, `second` and the station's spans) and the
  image's two, `relay` on the boot hart and `load` on hart 1, twelve an
  answer.
- **Counters, link, tracking:** the window's differences of the boot
  hart's counters, the bridge's frames, the frames the IP stack took, the
  cache's counts (cleared just before the window) and geometry, the
  receive driver's and the station's receive counts, the transmit
  agreements', the link's rates and the PHY tracking ticks.
- **Samples:** a run whose `Start` has a `profile` samples the boot hart
  every period over the window (`observe::Sampler`, 2048 samples), 27
  an answer; a sampler a session kept refuses the next sampled run
  (`NoSampler`).

A stop of the station closes the run's socket with the IP service (its
queued datagrams return their lent buffers); the run then receives and
sends nothing more and ends at its time.

## Regulatory configuration

Country `DE` (`oer-radio-regulatory`): the station scans every 2.4 GHz
channel the domain permits at each connect (`wifi::Config::all_channels`),
with 20 MHz width and an owner transmit cap of 10 dBm.

## Faults

A boot error, an application error and every unrecoverable station exit
record their named cause and reset the chip (the library's failure table);
a console transport fault ends the application with `sdk-console`. The
image builds in no credentials.

`cargo fw build esp32s31-sdk-station` builds it; the scenario's harness
builds and flashes it.
