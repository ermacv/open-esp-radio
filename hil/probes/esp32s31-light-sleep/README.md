# esp32s31-light-sleep

Whether the S31's HP system sleeps under the PMU and wakes at an LP-timer
target, with no domain powered down: the PLLs, the main crystal and the fast
RC oscillator stay on and the regulators keep their active output, so only
the CPU stalls (`oer-chips-family-drivers-pmu::sleep`, the HAL's
`power::sleep`).

The image is a staged boot with the boot hart's thread-mode executor, the
system timer and the RTC watchdog's feed; hart 1 stays held. The RTC
watchdog counts through sleep, so a sleep that never wakes ends in its
RTC-domain reset. It first prints the reset causes and what the PMU and the
LP timer hold of the last sleep request. Five seconds
after boot, while the host's console attaches, it reads the LP timer twice
200 ms apart, then arms the LP timer's first target 100 ms ahead and waits
for its wake status without sleeping. The RTC watchdog pauses in sleep, so
a sleep whose wake never comes would not end: no case sleeps unless the
target fired awake. Each case masks interrupts, reads the LP timer, sets its
target the case's length later at the slow clock's nominal 136 kHz, requests
sleep and reads the outcome.

| Line | What it reports |
| --- | --- |
| `ls boot reset<h>=<code>`, `ls pmu`, `ls wake`, `ls enables` | Each hart's reset cause, then the PMU's raw wake and reject status, causes and wake enables and the LP timer's target status and count |
| `ls counter ticks us hz` | LP-timer cycles over a pause of the system timer and the slow clock's rate |
| `target ms=100 fired rtc_ticks timer_us` | The awake target check: LP-timer cycles and system-timer microseconds until the target's wake status rose |
| `case=<n> clocks=<running\|gated> ms=<length> start` | The case, printed before it sleeps |
| `case=<n> outcome=<woke\|rejected> causes=<word>` | The PMU's outcome and its wake- or reject-cause word |
| `rtc_ticks` | LP-timer cycles from the request to the outcome |
| `timer_us` | System-timer microseconds across the same span |
| `rtc_hz` | `rtc_ticks` per second of `timer_us`: the slow clock's rate if the system timer ran through the sleep |
| `case=<n> error=<reason>` | The sequence's typed error: `bound`, `too-soon`, `counter` or `no-outcome` |

`running` keeps every clock gate the sleep state controls open; `gated`
closes them and runs last, since it may stop the USB console. A case passes
when it woke after at least its length in LP-timer cycles and before twice
it. `OER-RESULT: pass light-sleep` means the counter ran, the target fired
within one to two times its length and every case passed; `timer_us` and
`rtc_hz` are reported, not judged.

Run it with `cargo fw run esp32s31-light-sleep --board <board>` under a
lease.
