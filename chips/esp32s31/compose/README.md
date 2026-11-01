# ESP32-S31 image composition

`Esp32s31` binds the family executor and staged adoption to the S31 HAL port.
Images choose their interrupt table, task set, credentials and terminal policy.

`platform` owns chip-specific runtime preparation, the factory identity and
entropy owner, paired P2/P1 timer installation, interrupt part types, the trace
clock and the unanswered-request reset binding. Its methods consume the actual
owned partitions; they do not claim replacement hardware or hide a fallback.
`phy_fault::Failure` carries a copied failure observation to an image-selected
retained reset, without exposing the HAL's fault type in the Wi-Fi API.

`exception::capture` is the exception hook every S31 image passes to
`entry!`. The runtime has already retained the trap frame before it stopped
the other hart; the hook adds only what it reads afterwards: the trapping
hart's pending sources, `mtvec` and `mtvt`, and, for an interrupt of a line
without a handler, the routes to that line and its state, which it reads
only when the stop held. It allocates, waits and panics nowhere.
`exception::report_previous` prints the record at the next boot and clears
it only after every note was written whole; `peek_previous` reads it first
without clearing, for an image that serves it. `report_stack_guard` prints
the task stack's watchpoint.

`secondary::launch_hart1` starts hart 1 during boot under its own deadline
on the time driver's clock: it takes the installed time driver's
`TimeDriverReady` as the proof that the counter latch is checked and
published, so an image initializes its timers, and may grant hart 1's
executor its comparator (`oer_espressif_executor::grant`), before the
launch hands hart 1 its parts. The image's composition places hardware on
hart 1 by moving it into those parts: boot-smoke moves `AesParts` and builds
`crypto::key_unwrap` there over hart 1's router's `Silent` proof, which
holds because every hart's router silences its own bank at install; images
that do not launch hart 1 keep AES on the boot hart.

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

`sampler::Storage<I, O>` is a program-counter sampler on TIMG1 T0 for an
image that binds its entry `I` above the levels it wants to see and names
the observer `O` of its `oer_trace::profile::PcSample`s. Between `start`
and `stop` the timer runs an absolute schedule: tick `k` is due
`(k + 1) * period` after the start on the timer's own counter, and each
tick moves the alarm one period on without a reload, so handler latency
shifts no later tick; a tick due under masked interrupts fires where they
open. Each scheduled tick reports one sample of the interrupted `mepc` and
`mcause.mpil`; a tick that finds a stop request reports none, stops the
timer and hands the session back. Starting hands the timer's program part
and binding to the handler through a one-slot queue with the hart's
interrupts masked; the task and the handler share only the queues, a stop
flag and a stray count. Idle, the timer is stopped and masked.

The composition has no HIL dependency, credentials, reconnect policy or test
verdict. Instrumentation remains an image cfg, with no diagnostic counters in
the product path.
