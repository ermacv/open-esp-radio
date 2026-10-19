# ESP32-S31 HIL target

- [System and PHY watchdogs](watchdogs.md)
- [Stack measurements](memory.md)
- [Host setup and fixture installation](../../../tools/hil/run/HOST.md)
- [Wire and evidence contracts](../../../protocols/hil/protocol/README.md)

This workspace selects the shared board boot and memory profile and owns the
Embassy executor, the USB Serial/JTAG transport and the HIL workloads of the
radio-free system images. Effective dependency locks are archived beside each
image. No radio image is here.

## Application boundaries

The agent binary is the HIL application of the shared stage-two boot
contract. Each image class selects one base feature: `system-watchdog`,
`system-panic-reset` or `boot-smoke` (`usb-jtag-off` adds to
`system-watchdog`). Board initialization, relocation and interrupt-stack
mechanics belong to the shared platform; an application image would use the
same mechanism through `oer-image-build`. A hardware scenario verdict remains a
separate HIL responsibility.

## Protocol families per image

Each image enables only the HIL protocol families it serves
([radio families](../../../protocols/hil/protocol/README.md#radio-families)): the radio-free
`system-watchdog`, `diagnostic-usb-jtag-off` and `system-panic-reset` images
read only the shared core, `system` module included.

## Panic and exception records

The agent's panic hook, which the platform's panic entry calls through its
`panic-hook` feature, runs in every image but `system-panic-reset` (the
agent's `panic-hook` feature, which each image's base feature enables); that
image runs the product panic entry alone, so the `system-panic-reset`
scenario exercises the product's record-and-reset path. The hook and the
exception entry format nothing: they record the post-mortem fault and the
hart's trap registers, the vector it took and the sources pending and routed
to it (`agent/src/fatal.rs`) in RTC fast memory and reset, and the next boot
prints `OPEN_RADIO_HIL runtime=PANIC boot=previous` or `runtime=EXCEPTION
boot=previous` with that state before anything else. A panic message with
arguments is not kept, only a static one in task context. An
interrupt-context panic omits location and message because compiler metadata
may be in unavailable PSRAM; the hook receives the platform's safe
`PanicDetails`. The hook's code and diagnostic stage stay in SRAM, its
records stay in RTC memory, and its fault CRC uses no cached lookup table.

