# Linux network fixture

Use the Linux fixture software installation
route from the repository root:

```console
cargo stand fixture install --provider linux-net --dry-run
cargo stand fixture install --provider linux-net
```

Run Cargo as the unprivileged operator; it requests sudo only for installation.

Preparation requires Linux, Cargo, a C compiler, make, pkg-config, libnl3 and
OpenSSL development files, curl, tar and patch. It builds the pinned
[hostapd input](hostapd/README.md), the pinned udhcpc client, the finite probe
helper and the versioned network helper without privileges. Interactive sudo
is used only for the final root-owned staging, policy validation and activation.

The provider is deliberately restricted to `wlan0`. Installing it does not
discover or assign adapters, change network state, start hostapd, reset rfkill,
open serial/SSH, flash a DUT or transmit RF. Those effects belong to explicit
fixture checks and the scenarios that lease the station equipment.

## Station contract

The `client` operation accepts one JSON profile on stdin: SSID bytes as
`ssid_hex`, `security` (`open` or `wpa2`), `passphrase` (absent for open),
`frequency_mhz`, `phy` (`ht` or `non-ht`), and `ipv4` (`mode` is `none`,
`static` with `address` as CIDR, or `dhcp`). It prepares the private
supplicant with its network disabled. The host subscribes to the private
control socket before enabling that network.

The foreground `client-dhcp` operation maintains the station lease and
prints decoded lease state. Its stdin accepts `status`, `ping` and
`disconnect`. Ping sends 100 echoes at one-second intervals to the DHCP
router, while the station also answers the router's reverse ping. The
session closes on disconnect, EOF, cancellation or its bounded lifetime,
releasing its DHCP child and removing the station IPv4 configuration.
`stop` also removes station IPv4 state. The interface remains excluded
from NetworkManager throughout fixture ownership.

Example profile (fictitious SSID):

```json
{"ssid_hex":"4c6162","security":"open","frequency_mhz":2437,"phy":"ht","ipv4":{"mode":"dhcp"}}
```

Send the profile over stdin to `sudo -n /usr/local/sbin/open-radio-net client`.
After association, start `sudo -n /usr/local/sbin/open-radio-net client-dhcp`
with piped stdin/stdout. It emits one JSON object per line:

```json
{"event":"bound","lease":{"address":"192.0.2.8","prefix":24,"router":"192.0.2.1","server":"192.0.2.1","obtained_at":1000,"expires_at":4600}}
{"event":"ping","ping":{"sent":100,"received":100,"interval_ms":1000}}
{"event":"disconnected"}
```

`status` emits `event=lease` with the current lease. `ping` is admitted once
per session; its only destination is the DHCP router. Any lost echo remains
visible in `received`. The helper uses the fixed interface, count and
interval; stdin cannot supply a destination, command, path or ping flags.
Lease timestamps use seconds on Linux's shared `CLOCK_BOOTTIME` clock,
including suspend time and independent of wall-clock adjustments.
The station answers reverse echoes through Linux's ICMP responder throughout
the lease session. A router-side ping can run concurrently with the station's
ping. The caller verifies its independently captured echo pairs.

DHCP acquisition is bounded to 30 seconds, then the session to 240 seconds.
Renewals preserve the acquired address without a configuration gap. NAK,
lease loss/expiry, address or router changes during ping, malformed options
and DHCP child exit fail the session and trigger cleanup. DNS and the host's
default route are untouched; only the station address and its connected
route are installed. Explicit disconnect, EOF, cancellation and `stop` also
release the DHCP child and deauthenticate the station. `managed` restores
the fixture's ordinary managed-interface/rfkill state, with the permanent
NetworkManager exclusion still in place.

The DHCP binary is BusyBox 1.38.0 with only its `udhcpc` applet. BusyBox is
GPL-2.0-only, built locally from verified source and installed only on this
host; it is not committed, included in CI artifacts or redistributed.
Its source archive is verified by SHA-256, with the build configuration in provenance.
The [upstream-derived source hash](https://raw.githubusercontent.com/buildroot/buildroot/master/package/busybox/busybox.hash)
is pinned in `oer_lab_fixture::udhcpc`. Build it without root or RF:

```console
cargo stand fixture build-udhcpc
```

The runtime binary and provenance are installed with the other artifacts
of one generation. Source/configuration and cached build outputs live under
`target/hil/udhcpc`; no system DHCP package or NetworkManager takeover is
required. Schema 16 requires the station JSON profile and an exact capability
check before operating the fixture. Callers retain their fixture leases until
the foreground session exits and allow 20 seconds for its cancellation cleanup.
