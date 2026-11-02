# Devices

The device group owns board identity and the host operations on a board.
The LAB service (`oer-tools-lab-service`) is their one caller on the stand: it
owns every board, so no operation here takes a lock.

| Package | Boundary |
| --- | --- |
| `devices/` (`oer-tools-lab-board`) | Board operations as modules: `discovery`, `port`, `console`, `reset`, `openocd`, `flash`, receipted `image` writes and the stand file's board with every operation on it (`stand`) |
| `oer-tools-lab-model` | `DeviceId` and its canonical MAC form; what is known of a board apart from its hardware: reset paths, recovery steps and a board's rungs, the ROM's reset lines (`rom`), the reference peers' line grammar (`peer_line`) and `uhubctl`'s report (`hubs`); the clients of the LAB service use them without the board operations |

The LAB service uses the modules directly, for example
`oer_tools_lab_board::console` or `oer_tools_lab_board::reset`; its clients do not
build against this crate.

`image::write` takes the board's `DeviceId`, reads a verified bundle
snapshot once, invalidates the previous receipt before the write and
publishes a new receipt only after the whole write succeeds.

Keep operations inside `oer-tools-lab-board` unless a separate consumer needs a
smaller dependency boundary or a measured compilation benefit warrants a
package. A single operation does not by itself warrant another crate.

Run device regressions with `cargo test -p oer-tools-lab-board`. Identity and
peer grammar tests live in their respective packages.
