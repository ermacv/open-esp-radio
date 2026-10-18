# HIL flash operation

`oer-hil-flash` is the only way the host writes a board's flash:

1. **lease**: the caller holds the board's lock (`oer_hil_arbiter::lock::
   BoardLock`), taken under the arbiter's grant; a lock of another board is
   refused;
2. **write**: the stand board (`oer-stand-board`, through `oer_devices::image`) writes the image bundle;
3. **journal**: the board journal records the image the board now carries,
   with the SHA-256 of the bundle's application computed here;
4. **start**: the image starts as the chip profile says, unless the write's
   own reset started it.

`flash` does all four; `flash_if_changed` first asks the journal whether the
board already carries the image with that digest. `catalog::flash` builds an
ESP-IDF catalog image, bundles it with its own bootloader and partition
table and flashes it. The callers are the runner's flash of its device under test and of the reference
peers, and the calibration cross-check. A board is reached through the
`Target` trait, which board I/O's `Board` implements and the tests fake.

```console
cargo test -p oer-hil-flash
```
