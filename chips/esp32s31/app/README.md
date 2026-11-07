# ESP32-S31 application entry

The chip's implementation of the application entry
[`oer`](../../../oer/README.md): the boot sequence, the fixed interrupt layout,
the executors and service tasks, and the peripherals, drivers and services an
image takes. An image names no chip; `oer` re-exports this crate for an image
built for the ESP32-S31.
