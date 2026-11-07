# ESP32-S31 application entry

The chip's implementation of the application entry
[`oer`](../../../oer/README.md): the boot sequence, the fixed interrupt layout,
the executors and service tasks, and the peripherals, drivers and services an
image takes. An image names no chip; `oer` re-exports this crate for an image
built for the ESP32-S31.

`pilot::Storage` holds the permanent affine SAR/PWDET request/reply cells,
handler and signal. An image places it in critical data, selects its PHY and
radio-executor entries and calls `init` once before enabling interrupts. The PHY
driver stops/masks the timer and discards stale RAW before binding. Interrupt
service returns custody through the same P2 signal; neither a task waker nor a
hardware owner crosses into the handler. Existing retained faults and explicit
reset policy apply to setup failure and an orphan alarm.

`execution-inventory.toml` lists what the entry's execution consists of today:
interrupts, executors, task sets, signals, timers, the hart start and the known
waits, each with its cfg, its checks and what nothing checks yet. A layer test
holds the list to the code.
