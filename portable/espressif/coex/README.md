# Espressif coexistence policy

`oer-espressif-coex` is the coexistence policy the Espressif chips share,
recovered from esp-coex-lib `c758e7b5`: the 49-event priority table
(`CoexPtiTable`, equal in the pinned ESP32-S31 and ESP32-C5 archives) with
the IEEE 802.15.4 levels it resolves, the timer programming sequence, clock
conversion, event-to-timer mapping and software schedule state. It owns no
register: each chip's radio domain keeps its own copy of the table as
shared state, and a chip crate such as
[`oer-esp32s31-radio-coex`](../../../chips/esp32s31/radio/coex/README.md) binds the
`CoexTimerHardware` and `CoexClockHardware` ports to its HAL.
`CoexTimerHardware::pti` reads the arbiter's table when a request is
programmed, and a priority change goes through the arbiter's lease. The
ESP32-S31 arbiter lends only timers 0 through 4: timer 5 carries its PHY
grant-protect request, so event 48 has no policy timer. No runtime combines
the core and the schedule now: the former ESP32-S31 radio runtime did, with exclusive
access to the shared radio, and its policy is recorded below
([Participants, status and order](#participants-status-and-order)). This
crate does not implement
RF grant notification. See the ESP32-S31
[source capabilities](../../../chips/esp32s31/radio/coex/FEATURES.md) for those
boundaries.

The recovered values cite the ESP32-S31 archive where they were reviewed;
the PTI table and the IEEE 802.15.4 level lookup were also reviewed against
the ESP32-C5 archive.

## Portable clients and priorities

The portable vocabulary is [`oer-radio-coex`](../../radio/coex/README.md).
`CoexClient` converts from its `RadioClient` into the vendor request kind
(IEEE 802.15.4 has none), `CoexStatusType` into the status word a client
publishes, and `Ieee802154CoexLevel` from and into its `CoexPriority`
(`Idle`, `Low` = `Normal`, `Middle` = `Elevated`, `High` = `Critical`).

## Time-slice schedule

`CoexSchedule` is the recovered `coex_schm_env` of esp-coex-lib `c758e7b5`.
Each radio publishes status bits (`set_status_bits`, `clear_status_bits`);
the status words select one of the 107 recovered schemes of
`coexist_scheme.o`, exactly as `coex_schm_status_change` does. The compiled
schedule was compared with the entries of the pinned `libcoexist.a`
([vendor pins](../../../knowledge/esp32s31/vendor/artifacts.toml)). A scheme
divides a period into phases; a phase lasts period × interval × share
microseconds and notifies Wi-Fi, Bluetooth or both. The schedule programs no
priority itself: a notified radio requests its own events through
`CoexCore`, and the hardware arbitrates by priority.

The phases loop only while at least two radio groups publish status. With
Wi-Fi scanning, connecting or keeping only a connectionless window, they loop
on the schedule's own timer; a connected Wi-Fi stops them at the last phase
and restarts them itself (`restart`) at its beacons. Each `CoexPhaseStep`
tells the runtime owner how long to arm the phase timer and whom to notify.
`CoexScheduleExecutor` turns each step into a phase-timer command with a
generation; an expiry of an older generation lost a race with a later phase
change and steps nothing. The former radio runtime ran the timer and
signalled each notified radio; no runner exists now. A radio publishes its status through the schedule's
`set_status_bits` and `clear_status_bits`; in the former runtime Wi-Fi
reacted to its phases, while Bluetooth LE did not wait for its own.

On the ESP32-S31, `oer_esp32s31_radio_coex::CoexDomainPorts` lends one arbiter
lease's timer bank, event priorities and clock to `CoexCore` as its timer and
clock ports.

## Event requests

`CoexCore::request_wifi` and `request_bluetooth` return a programmed timer
identity. Latency and duration are source parameters converted into timer
targets; they are not guaranteed RF start/end times. `CoexStatus::active_timers`
reports requests retained by software, not current RF ownership.

## Failed hardware transactions

The hardware trait permits an error after a register write. Before programming
a timer, the core records an uncertainty bit. It clears this bit only after
configuration, both clock conversions/target writes and enable have succeeded.
Errors from any of those steps retain the affected timer for cleanup, even if
it never entered `active_timers`.

While `uncertain_timers` is nonzero, further requests return `RecoveryRequired`
without issuing hardware operations. Calling `enable` cannot clear this
condition. `release` or `disable` executes the backend withdrawal transaction;
failure retains uncertainty for a subsequent explicit recovery attempt.
Successfully retired timers are removed from software bookkeeping.

`disable` visits both successfully programmed and uncertain timers. It changes
the core to disabled only after all required withdrawal transactions succeed.
The Embassy adapter therefore cannot return `Stopped` after a failed shutdown;
its owner remains available to report status and accept a recovery command.
No automatic retry loop or synthetic RF grant is introduced.

This is accounting for hardware transactions, not proof that a frame,
descriptor walker or another protocol has stopped. The arbiter ports perform
the reviewed register sequence; their successful return must not be promoted
to whole-radio quiescence. Direct
timer access outside the core requires its own ownership and cleanup contract.

## Ownership boundary

The core holds values and cleanup obligations, while the supplied HAL owns
register access. Both must stay associated for their full control epoch.
The standalone Wi-Fi PHY maintenance capability is a separate HAL boundary;
a timer index or mailbox reply cannot be converted into that capability.

Three similarly timed operations remain separate contracts. Wi-Fi
peer-facing power save exchanges PM state with an AP and governs peer buffering.
PHY tracking runs on the exclusively borrowed radio domain. Physical RF/baseband/clock
close follows the last client's release and the shared PHY lifecycle. A coex
timer request grants none of those authorities, and an immediate RF close/wake
cycle is not connected modem sleep.

IEEE 802.15.4 is not an inferred third numeric `CoexClient`. Its MAC PTI fields
and reviewed vendor wrappers have a distinct interface. `CoexPtiTable`
resolves its four coexistence levels (high, middle, low, idle) from table
events 41 through 44, which a radio publishes per operation scene.

## Participants, status and order

The former ESP32-S31 radio runtime ran this policy with exclusive access to the shared radio, as the pinned libcoexist does:

- **Participants (`coex_enable`/`coex_disable`).** The first participant
  selects the policy timers' clock (`coex_pre_init`) and enables requests
  (`CoexCore::enable`); every later one starts coexistence and wakes the
  radios' start waits. Before the last participant leaves, every programmed
  request is withdrawn (`CoexCore::disable`); with one participant left,
  coexistence stops. A withdrawal that fails keeps the participant in.
- **Active for a radio (`coex_status_get`).** Coexistence is active for a
  radio when at least two participants take part and another radio publishes
  schedule status (`CoexStatusWords`).
- **Status around an enable.** IEEE 802.15.4 enters coexistence and then
  publishes `schedule::IEEE802154_COEX_STATUS`
  ([`src/schedule.rs`](src/schedule.rs))
  (`esp_coex_ieee802154_status_enable`); leaving clears it first and publishes
  it again when the disable fails. External coexistence enables the
  participant and publishes `schedule::EXTERNAL_COEX_STATUS`
  (`esp_coex_external_set`); stopping clears it, withdraws the requests, then
  clears the priorities, disables the block and releases its module clock
  (`esp_coex_external_stop`).
- **What Wi-Fi reads.** Its power management reads, on every beacon and slice,
  `coex_status_get(WIFI)`, the schedule's current and flexible period
  (`coex_schm_curr_period_get`, `_flexible_period_get`), its interval
  (`coex_schm_interval_get`), the share of phase 0
  (`coex_schm_get_phase_by_idx(0)`) and the priorities of `pti::wifi_events`
  ([`src/pti.rs`](src/pti.rs)) `BEACON`, `SLICE` and `CONNECTION`
  (`coex_pti_get(0, 1, 46)`).
- **Order of IEEE 802.15.4 join and leave.** Joining prepares the shared PHY
  (`esp_phy_enable`), takes the BTBB reference and enters the PHY client
  set (`esp_btbb_enable`); leaving releases the PHY client and the BTBB
  reference, then closes RF when no client remains. Wi-Fi's modem sleep
  leaves the client set and closes RF when it was last; its wake wakes RF
  and re-enters the set (`wifi_rf_phy_enable`). The PHY transitions are the
  PHY crate's ([first and last PHY client](../../../chips/esp32s31/phy/README.md#first-and-last-phy-client)).
