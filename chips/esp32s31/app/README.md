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
waits. It records cfgs, source locations, existing checks and gaps. The 29
`number-site` rows distinguish 25 SWI uses, three observer indices and one
hart argument; the executor declarations are included in that count. Timer
installation distinguishes starting a radio executor from the fallback path,
which can also be taken with the `wifi` feature enabled.

The twelve known waits separate queue readiness, lent-buffer returns, network
stop, application/group send and receive, MAC facts and USB read and write.
Each names its conditions, retained owners, local bound (including absence of
one) and source evidence. `state = "read"` records a manual code reading, with
no guarantee of eventual service or absence of deadlock. A layer test checks
known declaration shapes in its listed directories and validates the evidence
locations; it does not prove those conditions or bounds.
