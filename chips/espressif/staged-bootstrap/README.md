# Espressif staged bootstrap

`oer-espressif-staged-bootstrap` owns flash entry, the application descriptor,
payload validation, relocation and the non-returning stage-two handoff.
It reaches the chip through `oer-espressif-platform-port`; the S31 HAL
implements that contract. The board crate supplies mapping adoption and
links no runtime.

The boot chain is ROM → ESP-IDF bootloader → flash bootstrap → stage two.
`cargo fw` encodes the pinned bootloader as DIO and the application as QIO,
using the chip's flash map. The bootstrap finds its payload through the flash
MMU, validates the stage-two header and CRC, copies PSRAM code and publishes
its caches before jumping. Peripheral owners do not cross that image boundary.

`startup::run` owns the cold-start order over the family's boot drivers.
The chip binding supplies a `BootProfile` (S31's `oer-esp32s31-boot::S31`);
the image supplies its layout, payload, tuning reference and SRAM scratch.
The root only splits owners. The runner installs the console and starts the
initial bounded timer latch check before creating the common boot deadline.
It then selects LP-fast, contains secondary harts and initializes APM/TRNG,
takes over watchdogs with RTC still armed, applies brownout policy, brings up
the CPU PLL and PSRAM, stages and checks CRCs, tunes flash, publishes code,
and disables RTC immediately before the jump. Staging precedes flash tuning.
All waits keep their existing bounds under the same boot budget.

The PSRAM driver owns its HAL controller/MMU adapter. Success returns a
private-constructor affine `MappedPsram<R>`; failure retains that same owner.
The chip adapter fixes its physical MMU window. Only consuming the successful
witness for the fixed HAL owner lends a static mutable slice, retiring every
remap authority first. Copyable range observations grant no loan. Staging
retains the payload loan through publication and handoff.

S31 brownout uses the sourced family `drivers::brownout::apply` and the chip
binding's mode 0, threshold 7, reset and flash/RF power-down profile. No
handler is installed and runtime does not reconfigure the detector.

All executable bootstrap code and ordinary constants stay in internal SRAM,
including out-of-line callees. Only explicit payload and tuning-reference
sections stay in flash. The linked-image audit enforces that placement;
the [staged layout](../staged-layout/README.md) defines the loader and
header contracts. Stage two adopts initialized memory without repeating cold
boot or changing its mapping.
