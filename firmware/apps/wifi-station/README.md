# wifi-station

`oer-firmware-apps-wifi-station`: a Wi-Fi station of its build profile, on
the application entry `oer-espressif` (`sdk/espressif`): it joins the
profile's network, takes an IPv4 address by DHCP and exchanges one line
over TCP with the gateway's echo port, again after every reconnection. It
declares no interrupt, DMA or service task of its own.

```console
OER_WIFI_STATION_PROFILE=/path/station.toml cargo fw build wifi-station --features station
```

The `station` feature takes the SDK's Wi-Fi. A profile without it, or the
feature without a profile, fails the build.

The variable holds the absolute path of a TOML file kept outside git:

```toml
ssid = "network"
security = "wpa2-personal" # or "open", without a passphrase
passphrase = "8 to 63 printable ASCII characters"
country = "DE"             # the regulatory domain
channel = 1                # 2.4 GHz, 1 to 13
cap_dbm = 10               # transmit power limit, 0 to 20 dBm
echo_port = 7              # the gateway's TCP echo port, 1 to 65535
```

Every field is required and nothing has a default: a missing or invalid field
fails the build with the field's name, never its value, and no profile falls
back to Open. The build makes the image's own configuration checks: the
country needs a regulatory domain in `oer-radio-regulatory`, and that domain
must permit the channel. The SSID and passphrase reach only the build's generated
constant. `profiles/open-ci.toml` is a neutral Open profile without
credentials; it checks the Open path only, and WPA2 needs a profile that
selects it.

Without `OER_WIFI_STATION_PROFILE` (and the feature) the image reports
`OER-RESULT: not-configured wifi-station`, takes no RF owner, enables no
radio interrupt and links no Wi-Fi code; the library only feeds the RTC
watchdog.

A configured image's boot checks the profile's domain, channel and power
limit before any radio owner starts, then starts the shared AES service and
the station; its second boot (`entry!(boot, hart1 = network)`) starts the
IP service with the library's defaults and runs the TCP exchange, which the
application asks for over a `Channel` with the lease's gateway. The application
connects with the profile's network and prints each failed attempt
(`OER-WIFI connect-failed` with the station's cause) before trying again
two seconds later; once joined, the station keeps the network and rejoins it
every second after a lost association. Each lease is printed
(`OER-WIFI ipv4`), a lease that does not come within 30 seconds prints
`OER-WIFI ipv4-timeout` and the wait goes on, and each exchanged line prints
`OER-RESULT: pass wifi-station` (`OER-WIFI tcp-failed` otherwise). A boot
error and every unrecoverable station exit record their named cause and
reset the chip; the next boot reports it.
