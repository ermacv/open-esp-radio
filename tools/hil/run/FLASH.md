# HIL flash operation

`oer-hil-run::flash` coordinates a HIL write through
`oer-lab-board::stand::LeasedBoard`:

1. The arbiter grant holds the board's `DeviceAccess`.
2. The leased board writes the bundle through `oer_lab_board::image`,
   publishes its receipt, starts the image and confirms the start.
3. The flash operation appends the image identity and revision to the board
   journal. The journal records history; receipts determine what is loaded.

`flash_if_changed` skips a board whose receipt matches the complete bundle.
`catalog::flash` builds an ESP-IDF catalog entry, bundles its own bootloader,
partition table and application, checks the target chip and writes it through
the same leased operation. A held catalog entry is refused. The runner's DUT
and peer writes and the calibration comparison use these functions. Tests
substitute the `Target` trait; real writes use `LeasedBoard`.

`cargo hil firmware list` lists catalog entries; `cargo hil firmware build
IMAGE` builds one without flashing. A scenario requiring a peer prepares its
catalog image automatically. There is no `cargo hil firmware flash` command.
`cargo fw flash IMAGE --board BOARD` writes a built S31 application bundle;
it does not select ESP-IDF peer catalog entries.

```console
cargo test -p oer-hil-run
```
