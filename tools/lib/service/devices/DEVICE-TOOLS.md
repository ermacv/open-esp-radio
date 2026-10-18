# Devices

The device group owns board identity, locking and host operations shared by
the dev kit, stand and HIL.

| Package | Boundary |
| --- | --- |
| `devices/` (`oer-tool-service-devices`) | Board operations as modules: `discovery`, `port`, `console`, `reset` and `openocd`; feature `image` adds `flash`, receipted `image` writes and the held-board `device` facade |
| `mac/` (`oer-tool-data-device-mac`) | `DeviceId` and its canonical MAC form, with only serde; stand files, journals and power records use it without serial I/O or process locking |
| `lock/` (`oer-tool-service-device-lock`) | Board ownership and delegation, using identity, durable state and processes; leases and workload contexts use it without serial I/O or espflash |
| `peer-line/` (`oer-tool-service-device-peer-line`) | The reference peers' line grammar, with no dependencies; CLI and protocol consumers need no board operations |

`oer-tool-service-devices` has no default features. A console or reset consumer uses its
modules directly, for example `oer_tool_service_devices::console` or `oer_tool_service_devices::reset`.
Image writers and consumers of `Device`, `Opened` or `devices()` enable
`features = ["image"]`. This keeps espflash, chip profiles and image bundles
out of the dependency graph of a consumer that only opens a serial console.

`image::write` takes a held `DeviceAccess`, reads a verified bundle snapshot
once, invalidates the previous receipt before the write and publishes a new
receipt only after the whole write succeeds. The facade and stand call the
same operation; the stand and HIL retain their own power and recovery policy.

Keep operations inside `oer-tool-service-devices` unless a separate consumer needs a
smaller dependency boundary or a measured compilation benefit warrants a
package. A single operation does not by itself warrant another crate.

Run device regressions with `cargo test -p oer-tool-service-devices` and
`cargo test -p oer-tool-service-devices --features image`. Identity, locking and peer
grammar tests live in their respective packages.
