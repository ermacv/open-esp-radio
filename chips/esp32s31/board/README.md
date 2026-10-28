# ESP32-S31-Function-CoreBoard-1

`oer-esp32s31-board` supplies the board's PSRAM window from generated
`LAYOUT`, its antenna-gain setting and conducted-power limits.
`BootstrapPsram` implements `AdoptsBootstrapMapping`: stage two records the
bootstrap's mapping without writing PSRAM, cache or MMU registers.
`PsramMapping` retains its address range for the image's lifetime; the
platform clock owner retains the corresponding MPLL reference.

The crate links no runtime or radio role. The
[bootstrap](../../espressif/staged-bootstrap/README.md) owns cold boot;
the [runtime](../../espressif/staged-runtime/README.md) owns stage-two
adoption, interrupts and retained fault reporting. Board/module facts live
in [platform](../platform/README.md); reset recovery belongs to
[host tooling](../../../docs/hardware-errata.md).
