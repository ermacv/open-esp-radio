# ESP32-S31 SDK station image

The image of the [SDK station scenario](../../sdk-station/README.md): an
application of the entry `oer-espressif` (`sdk/espressif`, features
`esp32s31`, `wifi` and `console`). Its boot takes the AES service, the Wi-Fi
station and its IP service with the library's defaults and the USB
Serial/JTAG console; the library places their interrupts, executors and
service tasks and records and resets on the image's behalf. The image
declares no interrupt, DMA or service task of its own. Its boot is the
[station example's](../../../firmware/apps/wifi-station/README.md) with
two differences: the `console` feature and `Console::open`, and every
channel scanned (`wifi::Config::all_channels`) in place of the profile's
one. The fault service is a handler, not a change to the boot. Review keeps the two boots in step; `oer-test-layers` checks only that
the image takes nothing the SDK places.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../hil-agent/espressif/README.md); the boot evidence is the library's (`Boot::evidence`): hart 0's reset cause, the previous boot's panic, and its CPU exception as the post-mortem (`Fault::Exception`), which the library already printed and cleared |
| [SDK station](../../services/sdk-station/src/lib.rs) | One worker on the application executor answers the requests in order; a request while one waits is refused as `Busy` |
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
