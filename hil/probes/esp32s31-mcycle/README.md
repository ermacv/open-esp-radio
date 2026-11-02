# esp32s31-mcycle

Whether the RISC-V cycle counter (`mcycle`) can serve as a cheap clock on
the ESP32-S31: what one read costs beside one read of the image's clock,
how fast it counts on each hart, whether it counts while a hart waits for an
interrupt, and how hart 0's and hart 1's counters relate.

The image is a staged boot with the boot hart's thread-mode executor, the
system timer and the RTC watchdog's feed. Hart 1 starts during boot and runs
no executor: it measures its own rate, then takes one sample on each of the
boot hart's requests through atomics in `hart_exchange`. A sample is the
first system-timer microsecond after the current one (the image's clock,
`EmbassyClock`, 1 MHz ticks) and the cycle counter read right after it.

| Line | What it measures |
| --- | --- |
| `read mcycle64` / `read now` / `read empty` | Cycles of 256 reads of `oer_riscv::cycle64`, of `EmbassyClock.now()` and of an empty loop, each through `black_box`, and per read |
| `rate hart=<h> busy` | Cycles over 10 ms of the system timer while the hart polls the clock (`per-10ms`) |
| `debugger hart=0 connected=` | Whether the executor's idle skips its wait for interrupt (an attached debugger) |
| `wfi direct hart=0` | A 1 ms timer deadline waited for with `wait_for_interrupt` directly in the task: timer microseconds, cycles, retired instructions and wakes |
| `wfi executor hart=0` | A 1 ms `EmbassyClock` wait of the task while the executor idles |
| `cross sample=a/b` | Both harts sampled at once, 1 s apart: hart 1's counter minus hart 0's, less the cycles of the time between the samples (`offset`) |
| `cross drift` | The change of `offset` over the second |

A counter that stops in a wait for interrupt shows fewer cycles per 10 ms in
the `wfi` lines than in `rate`. Equal counters on both harts show an
`offset` near zero; counters started apart show a constant `offset`, and a
non-zero `drift` means different rates. `OER-RESULT: pass mcycle` means
every case completed; it states nothing about the values.

Hart 1's wait for interrupt is not measured: it has no interrupt source.

Run it with `cargo fw run ../probes/esp32s31-mcycle --board <board>` under a
lease.
