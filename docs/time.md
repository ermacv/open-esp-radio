# Clocks, stamps and alarms

How time is represented across the stack. The vocabulary is in `oer-base-time`
(`crates/base/time`) and `oer-radio-port` (`crates/radio/port`); measured
facts about the Wi-Fi stamp are in
[rx-stamp](../knowledge/esp32s31/wifi/rx-stamp.md).

## Three kinds of time object

- A **clock** reads "now" in its domain and nothing else: `oer_base_time::Clock`
  for monotonic time, a port's `now()` for its radio domain.
- A **stamp** is a value the hardware recorded when an event happened. It
  carries its domain and the generation of its clock relation
  (`oer_radio_port::RadioStamp<D>`).
- An **alarm** wakes something at an instant of its domain. An executor wait
  is an alarm of monotonic time (`oer_base_time::Timer`). A hardware timer of a MAC
  or PHY (station TBTT, the TSF timers, the BLE modem LP timer) is an alarm of
  its own domain, armed through a port or driver operation, never an
  `oer_base_time::Timer`.

## Domains

| Domain | Counted by | Type | Relation to monotonic time |
| --- | --- | --- | --- |
| Monotonic CPU time | the system timer, through `oer-adapters-embassy-time` | `oer_base_time::Instant` | itself |
| ESP32-S31 Wi-Fi MAC local time | `WIFI_MAC_LOCAL_TIME` (`MacLocalTime`), widened by `MacTimeline` | `Ieee80211Instant`, `Ieee80211Stamp` | affine; its relation owner is the lower-MAC port backend (W3), generations `Ieee80211ClockGeneration` |
| An interface's TSF | the MAC's TSF of that interface | `TsfInstant`, as `VifTsf` with its interface | affine to the port's radio clock (`TSF_RELATION`, `TSF_DRIFT_PPM` = 200 ppm: the AP's timer and the station's crystal), sampled by `TsfSample` |
| BLE controller | the controller clock, extended from its 32-bit latch | `LeInstant` | none |
| BLE modem LP timer | the modem LP timer | `BluetoothModemLpTimerInstant` (wrapping, no claimed unit) | none |
| IEEE 802.15.4 | the runtime's monotonic clock | `Ieee802154Instant` | identical |
| FTM | the exchange's timestamps | `FtmTimestampPs`, 48-bit picosecond wire values | none |

A TSF generation (`TsfGeneration`) starts with the epoch each relation owner
is created with (`LowerMacConfig::tsf_epoch` for the S31 core), so two owners
never share one, and jumps at: a channel change, an interface
(re)configuration, an AP TSF restart, a station TSF set beyond the relation's
bound, and a TSF crossing 2^64 (the TSF is never treated as modular). A
sample of another generation does not convert (`StaleSample`).

## Rules

- A sans-IO protocol takes time as values of its own domain. A deadline it
  returns for the executor is a monotonic `Instant`; an instant it returns for
  a hardware alarm stays in that alarm's domain, and the driver arms it.
- Domains do not mix at the type level: a port's instants are
  `RadioInstant<D>` of its domain `D`, a coordinate in its owner's radio
  epoch. Raw microseconds establish no relation between clocks.
- Radio arithmetic belongs to `oer-base-time`: `RadioDuration<D>` spans add only
  to instants of the same domain. Checked operations return `None` outside
  the represented range; nothing wraps, narrows or clamps. `RadioWindow` is
  non-empty and half-open; touching windows do not overlap.
- Time crosses domains only through `ClockRelation<A, B>` (identical, or
  affine within a drift bound), a `RelationSample<A, B>` of back-to-back
  readings with its uncertainty and generation, and one projection both ways
  whose uncertainty is a span of the target domain. A port publishes its
  relation to monotonic time in `ClockInfo<D>::monotonic`, or `None`.
- Only a relation owner's `GenerationSource` makes a `Generation`.
- Time-exact work stays in its domain: the station follows the AP's TSF by
  the MAC local time between a beacon's stamp and now, not through monotonic
  time. A software handoff bound is monotonic: the HE Trigger and NDPA
  response windows count from the frame's executor handoff.
- The hot path only records: a received frame carries
  `RxTimes { handoff, stamp }` with the raw stamp, and no clock is read on
  reception. A consumer that needs the reception time converts the stamp.
- Hardware counters and alarms belong to their driver and reach the layers
  above as read-only capabilities (`MacLocalTime`) or port operations.
- Only adapters, the executor binding (`crates/exec/executor`, the time
  driver) and the firmware depend on `embassy-time`, dev dependencies
  included; every lower layer reads and waits through the `oer-base-time` ports,
  takes its timer from its owner or caller, and is tested with
  `oer-base-time-virtual`.
