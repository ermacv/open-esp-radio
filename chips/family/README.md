# The Espressif family

What every chip of the family shares. A crate here names no chip and depends
on no chip crate: it works over a port the chip's HAL implements or over
values. A directory exists here only for code proved neutral, so a chip may
hold a directory the family lacks.

| Directory | Holds |
| --- | --- |
| `facts/` | The layered chip facts and their family and ISA layers |
| `platform/` | Startup and execution: the platform port, the staged layout, bootstrap and runtime, the interrupt table, the executor |
| `boot/` | Cold-start sequences and runtime adoption |
| `drivers/` | One block each: regi2c, eFuse, PMU sleep, system timer, USB Serial/JTAG, watchdog |
| `radio/` | Radio power and clocks, PHY operations, coexistence, and under each protocol what its chips share |
| `app/` | What the application entry shares across chips |

The words are defined in [repository layout](../../docs/repository-layout.md#chip-crates).
