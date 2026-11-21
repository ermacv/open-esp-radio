# ESP32-S31 application entry

The chip's implementation of the application entry
[`oer`](../../../oer/README.md): the boot sequence, the fixed interrupt layout,
the executors and service tasks, and the peripherals, drivers and services an
image takes. An image names no chip; `oer` re-exports this crate for an image
built for the ESP32-S31.

The entry has no Wi-Fi role and no observation of its own in the built tree:
`wifi.rs`, `wifi/`, `pilot.rs` and `observed.rs` are files it does not
declare, which stood on the old Wi-Fi path (outside the build). Its features
are `console` and `group`.
