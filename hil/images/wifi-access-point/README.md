# ESP32-S31 Wi-Fi access-point image

The image of the [Wi-Fi access-point](../../scenarios/wifi-access-point/README.md)
scenario: an application of the entry `oer` (features `access-point` and
`console`; the build selects the chip). Its boot takes the Wi-Fi access
point (`wifi::AccessPoint::start`) with the static IPv4 configuration
`192.168.4.1/24`, no gateway, and the USB Serial/JTAG console; the library
places its interrupts, executors and service task and records and resets on
the image's behalf. Until a serve names a channel, the radio's regulatory
configuration is channel 1 of `DE` (`oer-protocols-radio-regulatory`) at a
cap of 20 dBm, above the domain's ceiling, which decides.

| Service | Handler |
| --- | --- |
| HIL base module | The [agent on the entry](../../agent/README.md); the boot evidence is the library's (`Boot::evidence`) |
| [Wi-Fi access point](../../services/wifi-access-point/src/lib.rs) | One worker on the application executor answers the requests in order |

- **Serve** builds the channel's configuration and the WPA2-Personal
  settings (`ApSettings::wpa2`: 100 TU beacons, DTIM 1) and starts the
  access point (`AccessPoint::serve`); a channel or setting the access
  point refuses answers `InvalidSettings`, a second serve
  `AlreadyServing`. The answer is `Beaconing` with the BSSID once the
  status counts a beacon, within 20 s, else `Timeout`.
- **GetStatus** answers the access point's status (`AccessPoint::status`),
  its IPv4 configuration and, once its task ended, how it ended
  (`AccessPoint::exit` as its line reads: `wifi-ap-end` and the exit's
  variant path).
- **Stop** asks the access point to stop (`AccessPoint::stop`) and waits
  for its task to end (`AccessPoint::installation_end`), within 10 s, else
  `Timeout`: it answers `Down` when the access point stopped and its radio
  was released (`ApExit::Ended(ApEnded::Stopped)`), otherwise `Ended` with
  the exit's line (a refused release names what the port still holds);
  before a serve it answers `NotServing`. The installation's end is kept
  for the boot.

No exit of the access point resets the chip. The image builds in no
credentials.

`cargo fw build hil-wifi-access-point` builds it; the scenario's harness
builds and flashes it.
