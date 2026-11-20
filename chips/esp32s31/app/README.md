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

`execution-inventory.toml` lists the entry's execution: interrupts, executors,
task sets, signals, timers, the hart start and the known waits. It records
cfgs, source locations, existing checks and gaps. Its layer test reads the
entry's source directory, so the rows of the undeclared files stay with them. The 29
`number-site` rows distinguish 25 SWI uses, three observer indices and one
hart argument; the executor declarations are included in that count.

The twelve known waits separate queue readiness, lent-buffer returns, network
stop, application/group send and receive, MAC facts and USB read and write.
Each names its conditions, retained owners, local bound (including absence of
one) and source evidence. `state = "read"` records a manual code reading, with
no guarantee of eventual service or absence of deadlock. A layer test checks
known declaration shapes in its listed directories and validates the evidence
locations; it does not prove those conditions or bounds.
