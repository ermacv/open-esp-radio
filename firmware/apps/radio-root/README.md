# radio-root

This ESP32-S31 staged application takes the reset root, adopts PSRAM, owns
`RadioHardware` from the HAL root's `into_runtime()`, and acquires and
releases each platform clock once through the chip's platform clock owner
(`oer_esp32s31_hal::power::ChipClocks`). It initializes no RF, PHY or MAC.
The radio root remains held until reset, and the image's one executor feeds
the RTC watchdog the root armed.

```console
cargo fw build radio-root
cargo fw run radio-root --board s31-c
```

The [typed boot console](../../../portable/diagnostics/README.md) reports any
previous panic first, then the clock results in `PlatformClock::ALL` order,
and the verdict. `cargo fw run` and `monitor` render the same text on
the host as before:

```text
OER radio-root clock=Pll160m acquired released
…
OER-RESULT: pass radio-root
```

A refused clock reports `clock=<name> refused=PlatformClockError` followed
by a failing clock verdict.
The shared previous-panic record keeps every retained field, including
whether the static message was absent or empty. No target formatter runs.
The records use `oer_trace::Event` without changing `portable/trace` or
allocating a trace ring. Sequence gaps, duplicate/reordered records,
unknown event types and malformed records are host errors.

The run's `log.txt` contains rendered text; `console.raw.txt` contains the
encoded console lines. `cargo fw decode-console <path>` replays them without
a board. A plain serial terminal sees `OER-DIAG:01…` records instead of the
former textual lines; decoded field names and clock order are preserved.
