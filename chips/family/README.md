# The Espressif family

What every chip of the family shares. A crate here names no chip and depends
on no chip crate: it works over a port the chip's HAL implements or over
values. A chip's tree has the same skeleton, and the HAL's modules follow it.

| Directory | Holds |
| --- | --- |
| `facts/` | The layered chip facts and their family and ISA layers |
| `platform/` | Startup and execution: the platform port, the staged layout, bootstrap and runtime, the interrupt table, the executor |
| `boot/` | Cold-start sequences and runtime adoption |
| `peripherals/` | One block each: analog bus, eFuse, PMU sleep, system timer, USB Serial/JTAG, watchdog |
| `radio/` | The shared domain, modem clocks, PHY operations, coexistence, and under each protocol what its chips share |
| `app/` | What the application entry shares across chips |

A block moves here when a second chip shares it or its logic needs no chip:
its sequences stand on a register port, and the HAL binds that port to its
clock, reset, interrupt and addresses.
