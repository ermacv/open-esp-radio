# Linux network fixture

Use the canonical [Linux fixture software installation](../run/fixtures.md#linux-fixture-software-installation)
route from the repository root:

```console
cargo stand fixture install --provider linux-net --dry-run
cargo stand fixture install --provider linux-net
```

Run Cargo as the unprivileged operator; it requests sudo only for installation.

Preparation requires Linux, Cargo, a C compiler, make, pkg-config, libnl3 and
OpenSSL development files, curl, tar and patch. It builds the pinned
[hostapd input](hostapd/README.md), the finite probe helper and the versioned
network helper without privileges. Interactive sudo is used only for the final
root-owned staging, policy validation and activation.

The provider is deliberately restricted to `wlan0`. Installing it does not
discover or assign adapters, change network state, start hostapd, reset rfkill,
open serial/SSH, flash a DUT or transmit RF. Those effects belong to explicit
fixture checks and HIL runs described in the [host operations guide](../cli/README.md).

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
