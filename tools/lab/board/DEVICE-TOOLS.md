# Devices

The device group owns board identity and the host operations on a board.
The LAB service (`oer-lab-service`) is their one caller on the stand: it
owns every board, so no operation here takes a lock.

| Package | Boundary |
| --- | --- |
| `devices/` (`oer-lab-board`) | Board operations as modules: `discovery`, `port`, `console`, `reset`, `openocd`, `flash`, receipted `image` writes and the stand file's board with every operation on it (`stand`) |
| `mac/` (`oer-lab-model`) | `DeviceId` and its canonical MAC form, with only serde; stand files, journals and power records use it without serial I/O |
| `peer-line/` (`oer-lab-board`) | The reference peers' line grammar, with no dependencies; CLI and protocol consumers need no board operations |

A console or reset consumer uses the modules directly, for example
`oer_lab_board::console` or `oer_lab_board::reset`.

`image::write` takes the board's `DeviceId`, reads a verified bundle
snapshot once, invalidates the previous receipt before the write and
publishes a new receipt only after the whole write succeeds.

Keep operations inside `oer-lab-board` unless a separate consumer needs a
smaller dependency boundary or a measured compilation benefit warrants a
package. A single operation does not by itself warrant another crate.

Run device regressions with `cargo test -p oer-lab-board`. Identity and
peer grammar tests live in their respective packages.
