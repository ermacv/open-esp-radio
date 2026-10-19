# Devices

The device group owns board identity, locking and host operations shared by
the dev kit, stand and HIL.

| Package | Boundary |
| --- | --- |
| `devices/` (`oer-lab-board`) | Board operations as modules: `discovery`, `port`, `console`, `reset` and `openocd`; feature `image` adds `flash`, receipted `image` writes and the held-board `device` facade |
| `mac/` (`oer-lab-model`) | `DeviceId` and its canonical MAC form, with only serde; stand files, journals and power records use it without serial I/O or process locking |
| `lock/` (`oer-lab-board`) | Board ownership and delegation, using identity, durable state and processes; leases and workload contexts use it without serial I/O or espflash |
| `peer-line/` (`oer-lab-board`) | The reference peers' line grammar, with no dependencies; CLI and protocol consumers need no board operations |

A console or reset consumer uses the modules directly, for example
`oer_lab_board::console` or `oer_lab_board::reset`.

`image::write` takes a held `DeviceAccess`, reads a verified bundle snapshot
once, invalidates the previous receipt before the write and publishes a new
receipt only after the whole write succeeds. The facade and stand call the
same operation; the stand and HIL retain their own power and recovery policy.

Keep operations inside `oer-lab-board` unless a separate consumer needs a
smaller dependency boundary or a measured compilation benefit warrants a
package. A single operation does not by itself warrant another crate.

Run device regressions with `cargo test -p oer-lab-board` and
`cargo test -p oer-lab-board --features image`. Identity, locking and peer
grammar tests live in their respective packages.
