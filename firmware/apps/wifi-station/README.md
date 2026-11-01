# wifi-station

`oer-firmware-apps-wifi-station`: a Wi-Fi station of its build profile, over
the ESP32-S31 station facade.

```console
OER_WIFI_STATION_PROFILE=/path/station.toml cargo fw build wifi-station
```

The variable holds the absolute path of a TOML file kept outside git:

```toml
ssid = "network"
security = "wpa2-personal" # or "open", without a passphrase
passphrase = "8 to 63 printable ASCII characters"
country = "DE"             # the regulatory domain
channel = 1                # 2.4 GHz, 1 to 13
cap_dbm = 10               # transmit power limit, 0 to 20 dBm
```

Every field is required and nothing has a default: a missing or invalid field
fails the build with the field's name, never its value, and no profile falls
back to Open. The build makes the image's own configuration checks: the
country needs a regulatory domain in `oer-radio-regulatory`, and that domain
must permit the channel. The SSID and passphrase reach only the build's generated
constant. `profiles/open-ci.toml` is a neutral Open profile without
credentials; it checks the Open path only, and WPA2 needs a profile that
selects it.

Without `OER_WIFI_STATION_PROFILE` the image reports
`OER-RESULT: not-configured wifi-station`, takes no RF owner and enables no
radio interrupt; it only feeds the RTC watchdog.

A configured image checks the profile's domain, channel and power limit
before any radio owner starts, then runs the radio actor and the Wi-Fi driver
on the P2 radio executor and the station on P1. The station task connects
once with the profile's network; the station keeps it and its reconnect
policy retries a failed or lost association every second. An ordinary `Stopped` exit ends the
station task. Every other exit and every handled construction error records
its named cause and resets the chip; the next boot reports it.
