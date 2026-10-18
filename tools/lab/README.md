# `oer-lab`

The stand, for the host tools that use it (`cargo fw` today): the owner's
stand file, the OpenWrt lab router that serves the stand's access point,
the SSH that reaches it, and a lock per router.

## The stand

- **Boards** on the ports of USB hubs whose port power `uhubctl` switches;
  each board's USB Serial/JTAG serial number is its chip's MAC.
- **The lab router**, an OpenWrt router whose 2.4 GHz radio serves the
  stand's access point (the AP the boards hear and, from W3, join).
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
protected = [3]              # ports that carry further hubs: never switched
switchable = [1, 4]

[[board]]
id = "s31-a"
usb-serial = "02:00:00:00:00:01"
chip = "esp32s31"
port = { hub = "top", port = 4 }

[station]
ssid = "made-up-stand-net"   # the stand AP's SSID
passphrase = "made-up"       # W4 (planned); never read today

[station_fixture]
kind = "open-wrt"
ssh_target = "lab-router"    # an ssh host or alias with a loaded key
wireless_interface = "phy0-ap0"
radio = "radio0"             # the UCI wifi-device section
ap_section = "default_radio0" # the UCI wifi-iface section of the stand AP
channel = 6                  # 1 to 13

[access_point]               # the board's own AP (planned); never read today
```

Unknown fields are ignored; each consumer deserializes only its fields, and
nothing else is read:

| Consumer | Reads |
| --- | --- |
| boards, flashing, power | `schema`, `[[hub]]`, `[[board]]` |
| W1 (`cargo fw run w1`, `cargo fw router`) | `[station].ssid`; `[station_fixture]` `kind`, `ssh_target`, `wireless_interface`, `radio`, `ap_section`, `channel` |
| W2, W3 (planned) | as W1, and what the probe and the open association need |
| W4 (planned) | also `[station].passphrase` |

**Secret** are the SSIDs, the passphrases, `ssh_target` and
`wireless_interface`; and every full MAC address the router reports. They
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
   `band`, `hwmode`, `channel`, `htmode`, `beacon_int`, `country`. `<radio>`
   must be a 2.4 GHz `wifi-device` (`band='2g'`, or `hwmode` `11b`/`11g`),
   `<ap_section>` a `wifi-iface` of it in mode `ap`, and no other enabled
   `wifi-iface` of the radio may carry the stand's SSID (the refusal names
   those sections only).

   ```sh
   config=$(uci show wireless) && printf '%s\n' "$config" | grep -E '^wireless\.[^.=]+=(wifi-device|wifi-iface)$|^wireless\.[^.=]+\.(device|mode|disabled|hidden|ssid|band|hwmode|channel|htmode|beacon_int|country)='
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
   config=$(uci show wireless) && printf '%s\n' "$config" | grep -E '^wireless\.[^.=]+=(wifi-device|wifi-iface)$|^wireless\.[^.=]+\.(device|mode|disabled|hidden|ssid|band|hwmode|channel|htmode|beacon_int|country)='
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
`$XDG_RUNTIME_DIR/oer-fw/router-<hash of the target>.lock` for the whole
`run w1` or `router`, and a second holder is refused, naming the first.
