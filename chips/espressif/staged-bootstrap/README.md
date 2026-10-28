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

Cold boot claims the HAL reset root and its bootstrap partition. It selects
the profile's LP-fast clock, contains secondary harts, initializes the SoC,
applies brownout policy, then brings up clocks, PSRAM, cache and flash.
All waits share the boot deadline below the armed RTC watchdog. Refused
mapping, flash tuning, readback or placement returns a typed boot failure to
the terminal boot reporter.

For S31, `Brownout::apply_idf_policy` selects mode 0, level 7, reset and
flash/RF power-down. No brownout handler is installed; interrupt enables stay
as found. Readback checks mode, actions and waits, excluding the analog
threshold byte. This runs before
radio access; runtime does not reconfigure the detector.
SOURCE: ESP-IDF 4d59230d `esp_brownout_init`; the sourced sequence lives in
`chips/esp32s31/hal/src/boot/brownout.rs`.

All executable bootstrap code and ordinary constants stay in internal SRAM,
including out-of-line callees. Only explicit payload and tuning-reference
sections stay in flash. The linked-image audit enforces that placement;
the [staged layout](../staged-layout/README.md) defines the loader and
header contracts. Stage two adopts initialized memory without repeating cold
boot or changing its mapping.
