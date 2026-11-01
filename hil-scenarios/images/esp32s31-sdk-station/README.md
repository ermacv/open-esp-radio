# ESP32-S31 SDK station image

The image of the [SDK station](../../sdk-station/README.md) and
[Wi-Fi station](../../wifi-station/README.md) scenarios: an
application of the entry `oer-espressif` (`sdk/espressif`, features
`esp32s31`, `wifi` and `console`). Its boot takes the AES service, the Wi-Fi
station and its IP service with the library's defaults and the USB
Serial/JTAG console; the library places their interrupts, executors and
service tasks and records and resets on the image's behalf. The image
declares no interrupt, DMA or service task of its own. Its boot is the
[station example's](../../../firmware/apps/wifi-station/README.md) with
three differences: the `console` feature and `Console::open`, every
channel scanned (`wifi::Config::all_channels`) in place of the profile's
one, and its own IP pools, 32 receive, 16 transmit and 32 general frames
(`Net::start_with` with a static `wifi::Pools`), in place of the library's
8, 8 and 8. The fault service is a handler, not a change to the boot. Review keeps the two boots in step; `oer-test-layers` checks only that
the image takes nothing the SDK places.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../hil-agent/espressif/README.md); the boot evidence is the library's (`Boot::evidence`): hart 0's reset cause, the previous boot's panic, and its CPU exception as the post-mortem (`Fault::Exception`), which the library already printed and cleared |
| [SDK station](../../services/sdk-station/src/lib.rs) | One worker on the application executor answers the requests in order; a request while one waits is refused as `Busy` |
| [Wi-Fi station](../../services/wifi-station/src/lib.rs) | The same worker, after the service's phases (`oer_services_wifi_station::phase`): one join per boot, a leave of the joined station once |
| [Fault](../../services/fault/src/lib.rs) | The same worker answers `RaiseException`, waits until the console wrote the answer and traps on an illegal instruction (`answer_then_trap`); a write the console cannot confirm ends the application with `sdk-console`, recorded and reset, without the trap |

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
- **Exchange** opens one TCP socket (128-byte buffers) on the IP stack,
  sends the line, reads as many bytes back and closes, within the request's
  timeout; without an address it answers `NoAddress`.
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
