# ESP32-S31 image composition

`Esp32s31` binds the family executor and staged adoption to the S31 HAL port.
Images choose their interrupt table, task set, credentials and terminal policy.

`platform` owns chip-specific runtime preparation, the factory identity and
entropy owner, paired P2/P1 timer installation, interrupt part types, the trace
clock and the unanswered-request reset binding. Its methods consume the actual
owned partitions; they do not claim replacement hardware or hide a fallback.
`phy_fault::Failure` carries a copied failure observation to an image-selected
retained reset, without exposing the HAL's fault type in the Wi-Fi API.
`radio_fault::Failure` (feature `radio-fault`) carries the shared RF actor's
failed periodic tracking, its fault class and variant path, to the same kind
of retained reset; the next boot prints it once its whole line was written.

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
`mcause.mpil` and, in an `oer_trace` build, right after it an
`oer_trace::profile::ReturnSample` of the interrupted `ra` that the staged
runtime's interrupt entry recorded (`stacks::interrupted_ra`; the sampler's
level is the hart's highest, so no entry runs between); in a leaf function
such as a memory copy it names the caller. A tick that finds a stop request
reports none, stops the timer and hands the session back. Starting hands the timer's program part
and binding to the handler through a one-slot queue with the hart's
interrupts masked; the task and the handler share only the queues, a stop
flag and a stray count. Idle, the timer is stopped and masked.

The composition has no HIL dependency, credentials, reconnect policy or test
verdict. Instrumentation remains an image cfg, with no diagnostic counters in
the product path.
