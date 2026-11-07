# ESP32-S31 layout

`oer-chips-esp32s31-platform-layout` is the staged-boot address map of the board. Its
build generates `LAYOUT` from the [chip facts](../../facts/README.md) and the
chip's stack policy, for the bootstrap, the board and every image's build
script. With the `build` feature, `build` links a binary with the shared
scripts, this chip's ROM script and the fragments of `linker/`:
`dma-window.x`, which the register model generates, and `hot-code.x`.

The chip-neutral contract is
[`oer-chips-family-platform-layout`](../../../family/platform/layout/README.md).
