# Platform HIL diagnostics

## Platform reset and watchdog

`GetBootStatus` returns `BootEvidence` for the boot identified by the
envelope. `ResetReason` is a platform value shared by all images: software
reset, MWDT1 reset or another cause; querying it does not change radio state.
`BootEvidence::platform_panic` is the record the platform's panic entry left
(`panic::take_previous`, taken once per boot): the panic's hart, whether it
ran on the interrupt stack and the interrupted PC, and the end of its file
path, line and column.

`lifecycle::RequestReset` is served by the panic-reset image and the system
watchdog image: the image acknowledges with `ResetAccepted`, waits until
the acknowledgement is written and requests a reset through the staged
runtime's retained `HilRequested` path. The next boot reports the request's
identity. This request does not invoke the panic hook.

The `system::PanicHook` property reports whether the image entry installs
the HIL panic hook. The panic-reset image has no hook; the watchdog and
USB-off images and the Wi-Fi station image have it. For an actual panic,
the watchdog image's hook also records the panic as the post-mortem fault
(`BootEvidence::post_mortem`) and, on the ESP32-S31, the fatal machine
state, which the next boot reports through the ROM as typed notes
(`OER-DIAG:01` lines) that the host renders as
`OPEN_RADIO_HIL runtime=PANIC boot=previous …`.

`system::DisableUsb` is served only by the `diagnostic-usb-jtag-off` image
(the system watchdog image with the image's `usb-jtag-off` feature): it
acknowledges with `UsbDisabled`, then drops the USB Serial/JTAG's D+ and D-
pull-ups and takes it off its pads. Off its pads alone, the device stops
answering but the host keeps it enumerated; without the pull-up the host sees
it disconnect. Only a reset of the chip brings it back. The host arms
`SerialCapture::expect_detach` before the request, so the end of the stream
that follows closes the capture without a failure.
The USB service also announces `system::UsbJtagOff`, so the host distinguishes
this variant from the ordinary watchdog image by a property key.

`system::WatchdogTest` is served only by the dedicated watchdog image.
The dedicated radio-free image's one-second engineering budget exercises completion, a synchronous
poll that never returns, cancellation, missing completion and late restoration
of the SoC deadline service. The correlated event acknowledges the selected
mode; only `Complete` acknowledges disarming. Other modes must produce a new
boot and MWDT1 reset reason. This does not inject a PHY failure or measure RF
cessation.

## Program-counter profile

No image samples the program counter: no image has the sampler
(`pc-profile`). The `telemetry` profile messages (`ProfileControl`,
`ProfileStatus`, `GetProfileSamples`) are in the protocol with no producer.

Sampling perturbs timing: a profiled run is diagnostic evidence, never a
throughput or timing measurement.

## Stack measurements

`StackUsage` carries task watermarks and optional dedicated IRQ watermarks for
both harts; a network session's evidence reports it. A hart that runs no
executor reports `None`. Images with shared task/IRQ stacks report `None` for
both dedicated IRQ fields.

IRQ sampling runs in thread mode with local interrupts masked during the SRAM
scan. It therefore adds a short interruption to RF servicing; these diagnostic
runs are not a timing baseline with instrumentation removed. The image's stack
policy supplies the nonzero required reserve. These are observed boot-lifetime
watermarks, not worst-case bounds or evidence of every possible nested IRQ.
