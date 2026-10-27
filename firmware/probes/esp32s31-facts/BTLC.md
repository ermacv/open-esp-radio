# Natural BTLC wrap observation

Selection 230 is the single 38-minute observation authorized on 2026-10-27.
Selection 231 checks the same reset-origin setup for 120 ms before committing
to that interval. Run under a held `cargo stand lease --board s31-a --air none`.
Other long runs or repetitions require a new owner decision.

The PAC-only leaf resets the digital BT MAC/APB/timer domains, uses the sourced
digital-clock prefix, selects CLOCK_SHIFT=3 and CONFIG_24=0, enables only the
primary overflow cause and writes CONTROL0=1. It never writes BTLC VALUE words
or TIMER_ARM. A bounded latch/advance check and XTAL / 1 CPU read-back are
required before the long interval. The CPU reference is nominal 40 MHz;
absolute oscillator calibration is outside this experiment.

MIE remains disabled. The ROM RTC watchdog stays armed and is fed through its
PAC owner on each bounded poll. Every approximately 10 ms the probe captures
the first BTLC word and primary/matrix status. It records raw decreases and
zero-to-positive primary transitions. These are observation diagnostics,
not the production wrap accumulator or a count of underlying hidden events.

The SRAM journal uses the existing 128-record buffer. Each observation is
three four-word records (tag followed by three values):

| Tag | Values |
| --- | --- |
| T | BTLC raw, mcycle-before low, mcycle-before high |
| T+1 | mcycle-after low, mcycle-after high, primary STATUS_1 |
| T+2 | complete matrix bank 3, complete matrix bank 4, NRT snapshot 1 |

Capture instants lie inside their mcycle brackets; status reads follow them.
The tags are: `e610` initial, `e620` after 120 ms, `e800+4*i` every 90 seconds
(i=1..25), `e700/e710` before/after raw decrease, `e720/e730` before/after
pending rise, `e740` before ack, `e750` after ack, and `e760` final.
`e600/e601` hold CPU root/divider and nominal reference/journal interval;
`e602` holds BTLC shift, CONFIG_24 and TIMER_ARM. `e74f` stores the actual
acknowledged affine image and observed wrap/rise counts; `e770` stores final
wrap/rise/ack counts. `e6fe=1` completes readiness; `e6ff` is a typed diagnostic
for bad reference, capture timeout or a failed advance/initial-status check.

No pending image is acknowledged between start and 2250 nominal seconds.
At that deadline one sampled primary image is consumed by its PAC affine
acknowledgement, followed by another capture. At 2280 seconds the final
journal and ordinary masked/cleared probe release complete the command.
The nominal natural wrap is 2147.483648 seconds at 2 MHz, leaving about 102.5
seconds of pending retention and 30 seconds of post-ack observation.
At most 115 records are emitted for one wrap/rise. Unexpected additional
transitions can exhaust the buffer and fail closed with the existing marker.

The host resumes the command once and does not halt it during the interval.
Read the final SRAM marker promptly: once the leaf returns, the ordinary ROM
watchdog backstop remains armed. Debugger accesses are limited to command /
result SRAM; the image performs every peripheral access through typed PAC.
No ISR, source-124 service count, production #713 continuity, independent
crystal comparison, RF behavior or second-period recurrence is tested.
