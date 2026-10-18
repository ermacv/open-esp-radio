# Embassy bindings

These crates bind Embassy facilities to driver contracts. Chip radio
execution belongs to the [radio runtime domain](../../runtime/README.md), which
holds no Bluetooth runtime since .

| Location | Responsibility |
| --- | --- |
| `esp32s31/executor/src/{executor,time_driver}.rs` | Platform executor wake ABI and Embassy timer queue; applications supply interrupt and timer capabilities |
| `radio/src/` | Mailbox and role-epoch actor binding the portable radio service port |

An adapter can retain state required by its external contract.

`esp32s31/executor` is the Embassy platform binding: executor wake-up and
timer-queue ABI. It has no radio policy or PHY initialization. Image time is
[`time`](time/)'s `EmbassyClock`, which chip PHY and the runtimes take as an
`oer_time::Timer`; chip PHY remains executor-independent.

Final memory profiles, static claims, IRQ binding and whole-radio lifecycles
belong to a chip composition; none exists since  (tag
`legacy-radio-2026-10-16`). Portable Wi-Fi execution primitives (monitor
handoffs, task shutdown, station network ownership) are runtime code in
[`runtime/ieee80211`](../../runtime/ieee80211/), not executor bindings. An
executor-free service never depends on an adapter. The [driver
map](../../README.md) defines the ownership direction.
