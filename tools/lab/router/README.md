# `oer-lab-router`

The stand, for the host tools that use it (`cargo fw` today): the owner's
stand file, the OpenWrt lab router that serves the stand's access point,
the SSH that reaches it, and a lock per router.

## The stand

- **Boards** on the ports of USB hubs whose port power `uhubctl` switches;
  each board's USB Serial/JTAG serial number is its chip's MAC.
- **The lab router**, an OpenWrt router whose 2.4 GHz radio serves the
  stand's access point (the AP the boards hear and join).
- **The host** on the router's LAN: it reaches the router over SSH with a
  key (no password prompt is ever answered).

## The stand file

`~/.config/open-esp-radio/stand.toml` (or `$OER_STAND_FILE`), one per host,
shared by every checkout. It holds the stand's credentials, so its mode must
be **0600**: any other mode is refused before the file is read. Schema 1,
with made-up values:

```toml
schema = 1

[[hub]]
id = "top"
usb2 = "3-8"                 # the hub's USB 2 location (`uhubctl -l`)
usb3 = "4-1"                 # its USB 3 companion
protected = [3]              # optional: ports never switched off
switchable = [1, 4]          # optional: when given, the only ports switched
keeps-power = [2]            # optional: ports that report off but keep power

buttons = { "1" = 1 }        # optional: the hub's touch button of a port

[stand]
id = "made-up-lab"           # the stand's identity, for HIL runs
air = "exclusive"            # a run that uses the air holds all of it

[[board]]
id = "s31-a"
usb-serial = "02:00:00:00:00:01"   # its MAC: how the board is found on USB
chip = "esp32s31"
radios = ["wifi-2g4", "ble"]       # HIL: a subset of the chip's radios
roles = ["dut"]                    # HIL: dut (our image) or peer (ESP-IDF)
reset = ["usb-jtag-rts", "jtag", "power"]   # HIL: the reset ladder, in order

[station]
ipv4 = { mode = "dhcp" }     # HIL: how the stand's station takes its address

[station_fixture]
kind = "open-wrt"
ssh_target = "lab-router"    # an ssh host or alias with a loaded key
wireless_interface = "phy0-ap0"
radio = "radio0"             # the UCI wifi-device section
ap_section = "default_radio0" # the UCI wifi-iface section of the stand AP
channel = 6                  # 1 to 13

[access_point]               # HIL: the board's own AP the station joins
channel = 6
channel_width = "ht20"
target_address = "192.168.77.1/24"
client_address = "192.168.77.2/24"

[air_observer]               # HIL: an idle OpenWrt radio observing the air
ssh_target = "lab-observer"
phy = "phy1"
interface = "mon1"

[bluetooth]
adapter = "hci0"             # HIL: the host's Bluetooth adapter

[legacy_bss]
country = "DE"               # HIL: the laptop's non-ERP BSS domain
```

The file holds no credentials: every access point and station of the stand
is managed, and a run generates the SSID and passphrase it sets on both
(`oer_lab_router::credentials`) and never logs or stores them. An SSID
or passphrase an older file still holds is never deserialized.

A board's hub port is not written in the file: it is found on the host
(`oer_lab_router::hub`). The board's USB location in sysfs names its hub and port
(`3-8.3.4.4` is port 4 of hub `3-8.3.4`), and `uhubctl` confirms that the hub
switches power per port and has that port. A port is switched off only when
the one device below it is the board itself (no hub, nothing behind it, and
nothing on the port's USB 3 companion) and the hub's `protected`,
`switchable` and `keeps-power` allow it. An older file's board `port = …` is
ignored with a warning naming the detected port.

Unknown fields are ignored; each consumer deserializes only its fields, and
nothing else is read:

| Consumer | Reads |
| --- | --- |
| boards, flashing, power | `schema`, `[[hub]]`, `[[board]]` |
| W1 (`cargo fw run w1`, `cargo fw router`) | `[station_fixture]` `kind`, `ssh_target`, `wireless_interface`, `radio`, `ap_section`, `channel`; the SSID is generated per run |
| the stand services and HIL (`cargo stand`, `cargo hil`) | also `[stand]`, the boards' `radios`, `roles`, `reset`, `startup-artifact`, the hubs' `buttons`, `[station].ipv4`, `[access_point]` without credentials, `[air_observer]`, `[bluetooth]`, `[legacy_bss]` |

**Secret** are the generated SSIDs and passphrases, `ssh_target` and the
interface names; and every full MAC address the router reports. They
appear in no message, error, `Debug`, lock file or run log: an SSID is shown
only as its length or as whether it matches, a MAC address only as
`xx:xx:xx:xx:F6:D0`. Channel numbers and UCI section names are shown. Do not
attach run logs, captures or the stand file anywhere.

## The lab router

The router is a lab router: before each W1 run (and on `cargo fw router`)
the tool brings it to W1's profile and verifies it. The profile is
committed and stays; the earlier settings are not restored. Other
`wifi-iface` sections are never changed.

Each step is one script that `ssh -o BatchMode=yes -o ConnectTimeout=5
<ssh_target> sh -s` reads from stdin, so no stand value but the target is in
an argument list; each takes at most 30 s. Values are single-quoted for the
remote shell. Every read is filtered on the router by an allowlist, so keys
(`key`, `sae_password`, `auth_secret`, `wpa_psk_file`, …) never leave it,
and the host refuses any line outside the allowlist. In order:

1. The staged wireless changes, as option paths. Any refuses the run: the
   profile's commit would take them along.

   ```sh
   changes=$(uci changes wireless) && printf '%s\n' "$changes" | sed 's/=.*//'
   ```

2. The wireless configuration: only `wifi-device` and `wifi-iface` section
   lines and the options `device`, `mode`, `disabled`, `hidden`, `ssid`,
   `band`, `hwmode`, `channel`, `htmode`, `beacon_int`, `country`, `encryption`,
   `ieee80211ax`. `<radio>`
   must be a 2.4 GHz `wifi-device` (`band='2g'`, or `hwmode` `11b`/`11g`),
   `<ap_section>` a `wifi-iface` of it in mode `ap`, and no other enabled
   `wifi-iface` of the radio may carry the stand's SSID (the refusal names
   those sections only).

   ```sh
   config=$(uci show wireless) && printf '%s\n' "$config" | grep -E '^wireless\.[^.=]+=(wifi-device|wifi-iface)$|^wireless\.[^.=]+\.(device|mode|disabled|hidden|ssid|band|hwmode|channel|htmode|beacon_int|country|encryption|ieee80211ax)='
   ```

3. The radio's channels in its regulatory domain: the profile's channel
   must be offered at 2.4 GHz and be neither `disabled` nor `no IR`.

   ```sh
   phy=$(ubus call iwinfo phyname '{"section":"<radio>"}' | jsonfilter -e '@.phyname') && test -n "$phy" && info=$(iw phy "$phy" info) && printf '%s\n' "$info" | grep -E '^[[:space:]]*\* [0-9]+(\.[0-9]+)? MHz \['
   ```

4. The profile: the stand's channel, HT 20 MHz, a 100 TU beacon interval,
   the radio enabled; the stand's SSID on the AP section, enabled, not
   hidden. Encryption and keys are never set.

   ```sh
   uci set 'wireless.<radio>.channel=<channel>' && uci set 'wireless.<radio>.htmode=HT20' && uci set 'wireless.<radio>.beacon_int=100' && uci set 'wireless.<radio>.disabled=0' && uci set 'wireless.<ap_section>.ssid=<ssid>' && uci set 'wireless.<ap_section>.disabled=0' && uci set 'wireless.<ap_section>.hidden=0' && uci commit wireless && wifi reload
   ```

5. The configuration again (the script of step 2): the committed values
   are the profile's, and the stand's SSID is on exactly one enabled
   `wifi-iface` of the radio.

   ```sh
   config=$(uci show wireless) && printf '%s\n' "$config" | grep -E '^wireless\.[^.=]+=(wifi-device|wifi-iface)$|^wireless\.[^.=]+\.(device|mode|disabled|hidden|ssid|band|hwmode|channel|htmode|beacon_int|country|encryption|ieee80211ax)='
   ```

6. The AP interface, up to 16 times 2 s apart (30 s), until it is an AP on
   the profile's channel, `width: 20 MHz`, with the stand's SSID; else the
   run is refused with the last reason. Its `addr` is the stand's BSSID.

   ```sh
   info=$(iw dev '<wireless_interface>' info) && printf '%s\n' "$info" | grep -E '^[[:space:]]*(addr|ssid|type|channel) '
   ```

A second run against a router already in the profile sends the same
scripts; nothing changes and the checks pass the same.

One process at a time drives the router: `cargo fw` holds an exclusive lock
`$XDG_RUNTIME_DIR/open-esp-radio/routers/router-<hash of the target>.lock` for the whole
`run w1` or `router`, and a second holder is refused, naming the first.

## W3 open station

`router::Profile::w3` also sets the selected AP's `encryption=none` and the
radio's `ieee80211ax=0`. The channel is the stand's 2.4 GHz channel, with
HT20 and a generated run SSID. The profile is committed without restoration.
It assigns the selected BSS to an isolated `10.231.0.0/24` network, with
OpenWrt at `10.231.0.1` as DHCP server, DHCP router and the sole ping endpoint.
The existing LAN, its management address and all other network sections are
preserved. A non-default route overlapping this subnet or staged changes in
wireless/network/dhcp/firewall refuse the profile before applying it.

The profile replaces only these named W3 sections and changes the selected
wireless radio/BSS. These are the complete settings it owns:

| Section | Settings |
| --- | --- |
| `network.oer_w3_device` | Empty bridge, no physical ports, `bridge_empty=1`, IPv6 disabled |
| `network.oer_w3` | Bridge device, static `10.231.0.1`, netmask `255.255.255.0`, `delegate=0`, `defaultroute=0` |
| `dhcp.oer_w3_dns` | Dedicated dnsmasq, bound only to W3 and excluding loopback; `nonwildcard=1`, `port=0`, `localuse=0`, `noresolv=1`, `nohosts=1`, `readethers=0`, `authoritative=1`, lease file `/tmp/oer-w3.leases` |
| `dhcp.oer_w3` | W3 interface and dnsmasq instance, start 100, limit 100, lease 15 minutes, `force=1`, `ignore=0`, DHCP option 3 = `10.231.0.1`, option 6 suppressed; RA, DHCPv6 and NDP disabled |
| `firewall.oer_w3` | W3 network/bridge only, input REJECT, output ACCEPT, forward REJECT, IPv4/IPv6 masquerade disabled |
| `firewall.oer_w3_dhcp` | Accept IPv4 UDP 68→67 from W3 to OpenWrt |
| `firewall.oer_w3_ping` | Accept IPv4 echo requests from W3 only to `10.231.0.1` |
| `firewall.oer_w3_out`, `firewall.oer_w3_in` | Reject all forwarding out of and into W3, placed first among configured rules |
| Selected radio | Stand channel, HT20, beacon interval 100 TU, enabled, HE disabled |
| Selected BSS | Generated SSID, enabled, visible, open encryption, W3 network, client isolation enabled |

There are no forwarding sections, NAT, LAN/WAN bridge ports or external DNS
service in the W3 configuration. The firewall is validated and reloaded before
the network is loaded. Network reload adds the W3 definitions without restarting
LAN; only W3 is brought up explicitly. Wireless is committed and the selected
radio is cycled to recreate its AP on the new bridge. Firewall is reloaded
with the active W3 device before verification; dnsmasq is reloaded after the
addressed bridge contains exactly the selected BSS. Runtime checks verify the bridge address and membership, firewall reject
rules and running DHCP instance. SHA-256 of every non-W3 network section must
be unchanged. The AX211 report lists all settings in `router_profile_changes`
and service actions in `router_service_actions`, redacting generated SSIDs and
interface names.

The DHCP instance uses OpenWrt's [per-instance DHCP
configuration](https://openwrt.org/docs/guide-user/base-system/dhcp_configuration#multiple_dhcpdns_serverforwarder_instances).

`w3::verify_open_ht20` checks the filtered runtime hostapd configuration:
open authentication, WPA off, HT on, no HT40 capability and HE off.

`w3::station` selects only the target's MAC in `iw station dump` and returns
presence/authentication/association/authorization flags. `w3::lease` selects
only that MAC from `/tmp/oer-w3.leases`, returning IPv4 and lifetime without
hostnames or client identifiers. The bounded in-memory ICMP observer and
router `ping` establish both 100-echo directions.
The capture ends naturally after all 400 ICMP rows, with up to ten seconds to
drain the final buffered batch and require a successful process exit and clean
capture statistics. A partial capture cannot establish either echo gate.
Both echo directions check adjacent request intervals of 1 s ± 250 ms,
without an absolute phase or a separate total-duration gate.
A fresh hostapd disconnect
event and the final station snapshot establish departure. Every process is
owned by the command and remote observers have a 240-second limit.

Callers hold router and exclusive air ownership for the complete run;
laptop work also holds its physical-radio and fixture-software leases.
The AX211 station belongs to the installed `linux-net` fixture, while
NetworkManager's unmanaged rule remains enabled. The strict firmware
contract and offline checker are documented in
[w3](../../../firmware/apps/w3/README.md).

## Stand notes

Facts of the stand's Linux hosts, from stand use 2026-09 to 2026-10 (records
outside this repository). `oer-lab-router` does not use them.

### Hub port power without sudo

The stand's hubs are a cascade of Realtek RTS5423/RTS5411 hubs (USB 2 and
USB 3 halves). This rule lets the `uucp` group run `uhubctl` on them and
nothing else; install it as `/etc/udev/rules.d/52-oer-uhubctl.rules` with
sudo and reload udev:

```udev
SUBSYSTEM=="usb", DRIVER=="usb", ATTR{idVendor}=="0bda", ATTR{idProduct}=="5423|5411|0423|0411", MODE="0660", GROUP="uucp"
SUBSYSTEM=="usb", DRIVER=="hub", ATTRS{idVendor}=="0bda", ATTRS{idProduct}=="5423|5411|0423|0411", RUN+="/bin/sh -c 'chgrp uucp $sys$devpath/*-port*/disable; chmod g+w $sys$devpath/*-port*/disable'"
```

- After a port returns, udev took up to 1 s to give it back to the group;
  a board took up to 15 s to re-enumerate after a reset.
- **Port 2 of the top hub** (USB 2 location `3-8`) reports `off` but keeps
  its power (checked 2026-10-02): mark it `keeps-power`. A power cycle there
  fails with "did not cut power". Not rechecked with a board in it.
- A wedged ESP32-C5 USB console (EPIPE on writes and on RTS/DTR, the device
  enumerated as `303a:1001` without string descriptors) recovered only by
  cycling that board's port (2026-10-03).
- There is no UART bridge to EN/BOOT: recovery is a port power cycle and a
  quick entry into USB-JTAG download mode.

### NetworkManager and the host radio

Linux fixtures (station, monitor, legacy BSS) own the host's Wi-Fi
interface, so NetworkManager must leave it. Permanently, in
`/etc/NetworkManager/conf.d/oer-unmanaged.conf`:

```ini
[keyfile]
unmanaged-devices=interface-name:wlan0
```

`nmcli -t -f DEVICE,STATE device` then shows `wlan0` as `unmanaged` (or
not at all). For a run only, a fixture writes
`/run/NetworkManager/conf.d/90-open-radio-hil.conf`, ran
`nmcli general reload conf` and `nmcli device set wlan0 managed no`, and
removes the file (and sets `managed yes`) on release:

```ini
[device-open-radio-hil]
match-device=interface-name:wlan0
managed=0
```

`nmcli device set … managed no` alone is not enough: it binds one netdev,
and every recreated `wlan0` is a new device that NetworkManager took back
and reconnected mid-handshake. A reconnect between `ip link set down` and
the nl80211 type or channel change gives `EBUSY` and leaves a monitor
interface without a channel.

### hostapd (Linux access point fixture)

- hostapd 2.12, pinned by SHA-256, with OpenWrt's `300-noscan.patch`
  applied unmodified.
- `noscan=1` only for a forced HT40 BSS; every other profile uses
  `noscan=0`. `noscan` skips the initial OBSS scan and all later
  coexistence actions; `ht_coex=1` stays explicit, since `ht_coex=0` alone
  does not skip the initial scan.
- Profile skeleton (SSID written as `ssid2=` hex; the passphrase never on a
  command line):

  ```ini
  interface=wlan0
  driver=nl80211
  ctrl_interface=<dir>
  ctrl_interface_group=network
  noscan=<0|1>
  ht_coex=1
  auth_algs=1
  wpa=2
  wpa_key_mgmt=WPA-PSK
  rsn_pairwise=CCMP
  ```

  DSSS: `hw_mode=b`, `supported_rates=10 20 55 110`, `basic_rates=10 20`,
  `wmm_enabled=0`. HT: `hw_mode=g`, `ieee80211n=1`, `wmm_enabled=1`,
  `ht_capab=[SHORT-GI-20]`, or `[HT40+][SHORT-GI-20][SHORT-GI-40]` (channel
  ≤ 9) / `[HT40-]…` (channel ≥ 5).
- A distribution hostapd may lack `CONFIG_DEBUG_FILE`: run it with `-d` in
  the foreground (`setsid stdbuf`) to keep the scan and coexistence
  decision in the log, and wait for `ENABLED` on the control socket.
- On OpenWrt, `beacon_int` and `dtim_period` were set as temporary UCI
  options and checked in hostapd's generated configuration.
- Repeated AP stop/start on the host's iwlwifi left a firmware peer context
  that put nothing on air; recreating the managed wdev
  (`iw dev wlan0 del`, `iw phy <phy> interface add wlan0 type managed`)
  cleared it.

### mt76 (OpenWrt router radio)

- A second, managed interface on the AP radio inherits the AP's address;
  mt76 creates the wdev but refuses to bring a duplicate address up. Give
  the client a locally administered address.
- Interface names are at most 15 bytes (IFNAMSIZ − 1); a longer one fails
  in nl80211 with "Attribute failed policy validation".
- Measured 2026-10-04/05: an OpenWrt mt76 client against the AP ran at
  HT40 MCS7, 135 Mbit/s; the laptop's iwlwifi client at 150 Mbit/s.

### wpa_supplicant (Linux station fixture)

- Wait (up to 2 s, polled every 20 ms) for the previous daemon to exit
  before starting one on the same interface; racing it gave an intermittent
  20 s scan with no frame sent.
- NetworkManager's global supplicant lives in `/run/wpa_supplicant`: use a
  private `ctrl_interface=DIR=<dir> GROUP=network` and remove the
  interface from the global one
  (`wpa_cli -g /run/wpa_supplicant/global interface_remove wlan0`).
- Global options `p2p_disabled=1` (a P2P wdev raced teardown on the Intel
  PHY), `mac_addr=0`, `preassoc_mac_addr=0` (no randomization); network
  block `disabled=1`, `key_mgmt=WPA-PSK`, `proto=RSN`, `pairwise=CCMP`,
  `scan_freq=<freq>`; for non-HT also `disable_ht=1`, `disable_vht=1`,
  `disable_he=1`. Before a run: `iw dev wlan0 set power_save off`.
- The passphrase never goes into argv (`/proc` shows it); Arch's
  `wpa_passphrase` needs a terminal even with piped input.
- Before signalling a stored PID on the router, check that it still
  belongs to `wpa_supplicant`.

### Linux Bluetooth peer

[linux-bluetooth.md](linux-bluetooth.md): adapter ownership, DTM,
connection and pairing quirks of a Linux host as the BLE peer.
