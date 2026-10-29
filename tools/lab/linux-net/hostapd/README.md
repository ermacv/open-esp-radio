# Linux fixture hostapd

`cargo stand fixture build-hostapd` owns downloading, SHA-256 verification,
patching, configuration, compilation and parser verification. `cargo stand fixture
install --provider linux-net` runs the same build before handing the terminal to
sudo. No prebuilt binary from
`/tmp` or an unrelated system package is used.

The source is [hostapd 2.12](https://w1.fi/releases/hostapd-2.12.tar.gz), pinned
by archive SHA-256 in `tools/lab/router/src/hostapd.rs`. The tracked `build.config`
enables nl80211, HT/VHT/HE, WPA2 and control sockets for this fixture.
`300-noscan.patch` is the unmodified OpenWrt patch included in this directory.
Its author and provenance remain in the patch header. It adds configuration
policy, not a bypass of the kernel's channel availability checks.

The build produces ignored `target/hil/hostapd/hostapd` and `provenance.json`.
The provenance records source/config/patch/builder hashes, compiler and library
versions, and the resulting binary hash. Reuse requires matching inputs and
binary hash. Build outputs use the current Linux compiler and system libraries;
this does not promise identical binaries across different host toolchains.
The installer copies provenance and hostapd into the same active root-owned
generation as the network helper; the stable path resolves through that
generation.

The helper selects `noscan=0` for normal coexistence. Explicit `force-ht40`
selects `noscan=1` for HT40 only. In this patch, `noscan` skips both the initial
OBSS scan and subsequent coexistence actions and client intolerance handling.
`ht_coex=1` stays explicit; setting only `ht_coex=0` would not skip the initial
scan. Neither mode relaxes the runner's actual channel/width validation.
