# ESP32-S31 image composition

`Esp32s31` binds the family executor and staged adoption to the S31 HAL port.
Images choose their interrupt table, task set, credentials and terminal policy.

`platform` owns chip-specific runtime preparation, the factory identity and
entropy owner, paired P2/P1 timer installation, interrupt part types, the trace
clock and the unanswered-request reset binding. Its methods consume the actual
owned partitions; they do not claim replacement hardware or hide a fallback.
`phy_fault::Failure` carries a copied failure observation to an image-selected
retained reset, without exposing the HAL's fault type in the Wi-Fi API.

`console::Binding` mounts the family USB driver on image-selected interrupt and
executor entries. The nominal `Console` implements async Read/Write, and `Pads`
retains the independent right to disconnect USB. Early output and lost-record
accounting keep the HAL's existing exclusive writer protocol.

`pilot::Storage` holds the permanent affine SAR/PWDET request/reply cells,
handler and signal. An image places it in critical data, selects its PHY and
radio-executor entries and calls `init` once before enabling interrupts. The PHY
driver stops/masks the timer and discards stale RAW before binding. Interrupt
service returns custody through the same P2 signal; neither a task waker nor a
hardware owner crosses into the handler. Existing retained faults and explicit
reset policy apply to setup failure and an orphan alarm.

The composition has no HIL dependency, credentials, reconnect policy or test
verdict. Instrumentation remains an image cfg, with no diagnostic counters in
the product path.
