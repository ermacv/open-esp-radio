# ESP32-S31 shared radio system

`oer-esp32s31-radio-runtime` owns the shared ESP32-S31 radio on the chip, as
ESP-IDF's `esp_phy` component does: the [radio arbiter](../../../hardware/esp32s31/hal/src/shared_radio.rs)
with its [shared PHY domain](../../../hardware/esp32s31/phy/src/concurrent.rs),
the platform token the PHY target port borrows and the platform sources of
the modem clocks.

## Use

`RadioSystem::new` splits the radio for concurrent clients and returns the
Wi-Fi, Bluetooth and IEEE 802.15.4 partitions for their compositions. The
protocol compositions are clients of the system:

- `RadioSystem::lock` takes the arbiter lease together with the platform
  resources, waiting while another holder owns it, as ESP-IDF waits for its
  PHY lock. The wait sleeps on a waker: each dropped `RadioGuard` releases
  its lease and then wakes every waiting task, which race for the lease
  again, so no release is lost and nothing polls the arbiter.
  `RadioGuard::parts` lends the lease, the platform token and the clock
  sources for one radio transaction.
- `RadioGuard::prepare_phy` is the first-client half of `esp_phy_enable`: it
  registers the shared PHY domain once, with the calibration identity given
  to `new` and the retained cache given to `RadioSystem::with_calibration_cache`,
  or wakes its closed RF. It reports `RadioPhyPrepared::Registered` with the
  registration's calibration path and fresh cache, `Woken` or `AlreadyOpen`.
  It needs no client's common power: the domain takes its own modem clock
  modules.
  The domain stays registered across RF close and wake, as ESP-IDF's
  calibrated PHY does, so only the first preparation registers.
- `RadioGuard::calibration_cache` captures the registered domain's current
  state as the cache for the next cold registration.
- `RadioGuard::close_phy_if_idle` is the last-client half of
  `esp_phy_disable`: it closes RF when no client remains.
- `RadioSystem::run_tracking` is the vendor periodic `phy_track_pll` timer.
  Every tracking period it runs one tick (`RadioSystem::track`): due
  tracking starts at once with the protocols running, as the vendor's does,
  and the tracking graph brackets its RF-sensitive regions with the
  grant-protect request. Run it for the lifetime of the radio;
  `RadioSystem::run_tracking_observed` also reports every tick's result, and
  `RadioSystem::run_tracking_until` ends when a stop future completes, which
  it polls only between ticks so a started tick always finishes.
  `RadioGuard::track_observed` runs one tick like `RadioGuard::track` and
  reports its operations to a `PhyTargetObserver`, for a diagnostic image
  that times them.

- `RadioGuard::suspend_wifi` and `RadioGuard::resume_wifi` are the vendor
  modem sleep's `wifi_rf_phy_disable`/`wifi_rf_phy_enable`: the Wi-Fi
  membership becomes a suspended token while Wi-Fi leaves the PHY client set,
  RF closes when no client remains, and a wake reopens RF before Wi-Fi
  re-enters. The registration and calibration stay across the sleep.

- Coexistence: `RadioGuard::enable_coex`, `disable_coex`,
  `request_wifi_coex`, `request_bluetooth_coex` and `release_coex` are the
  vendor `coex_enable`/`coex_disable` and request/release calls on the
  arbiter's timer bank. `RadioGuard::set_coex_status_bits`,
  `clear_coex_status_bits`, `set_coex_interval` and `restart_coex_phases`
  drive the recovered time-slice schedule (`RadioGuard::coex_schedule`).
  `RadioSystem::run_coex_schedule` is its phase timer: run it for the
  lifetime of the radio. Each phase change publishes the phase to
  `RadioSystem::wifi_coex_phase` and `bluetooth_coex_phase`, which keep only
  the latest unread phase.
  Each client takes part with the coexistence state the radio split issued
  it: `enable_coex` turns its `CoexAbsent` into a `Coexisting`, and
  `disable_coex` turns it back, so a client takes part at most once and
  leaves common power only while it shows its `CoexAbsent`. External
  coexistence takes part with the `ExternalCoexistence` of
  `start_external_coex`. The arbiter's participant set follows these states,
  as `coex_enable` counts radios: the second participant starts coexistence
  (`RadioSystem::wifi_coex_started`, `bluetooth_coex_started` with `true`),
  falling back to one stops it for Bluetooth, and the last leave withdraws
  every request. A leave whose timer cannot be withdrawn keeps the
  participant and returns its state (`CoexLeaveFailure`); a retry withdraws
  the requests again.
  `set_coex_wifi_channel` records the Wi-Fi channel for Bluetooth
  (`RadioSystem::bluetooth_wifi_channel`, `RadioGuard::coex_wifi_channel`),
  and `end_bluetooth_preemption` reports the end of a Bluetooth preemption
  to Wi-Fi (`RadioSystem::wifi_coex_preemption_end`). The schedule's period,
  interval, flexible period and phases are read through
  `RadioGuard::coex_schedule`; `RadioGuard::coex_active_for` answers
  `coex_status_get` for one radio. `RadioSystem::wifi_coex_view` is a cell
  Wi-Fi reads without the lease: every dropped guard refreshes its copy of
  Wi-Fi's coexistence status, the schedule's periods, interval and first
  phase share, and the priority of event 0. `enable_i_coex` and
  `disable_i_coex` are ESP-IDF's `esp_coex_wifi_i154_enable` and its
  reverse: IEEE 802.15.4 enables coexistence and publishes its schedule
  status; its MAC priorities follow its own operation scenes.

The Wi-Fi, Bluetooth and IEEE 802.15.4 compositions are all clients of the
system. Under Embassy, [`oer-esp32s31-radio-system`](../../../composition/esp32s31/embassy/radio/README.md)
creates it once and spawns `run_tracking` and `run_coex_schedule`.

## Joining the shared radio

Each protocol is one portable `RadioClient` of
[`oer-radio-coex`](../../../radio/coex/README.md) and joins the shared radio
through affine proofs, under one `RadioGuard`; a step without its proof
does not compile:

- **Common power.** The client's HAL owner enters it (`power_up`) and
  returns its `CommonPower<K>` proof; only `power_down` consumes it, and only
  while the client shows its `CoexAbsent<K>`.
- **The shared PHY.** `RadioGuard::prepare_phy` registers the domain or
  wakes its closed RF. `phy_ready` of `oer-esp32s31-phy` then proves the
  domain ready (registered, RF open, settled) by borrowing the lease, and
  `join_<client>` consumes that `PhyReady`: it enters the PHY client set,
  returns the client's affine `<Client>PhyMembership` with its
  `ConcurrentAcquire` (whether tracking is due before RF use) and gives the
  lease back. `leave_<client>` consumes the membership, and after the last
  client left `RadioGuard::close_phy_if_idle` closes RF.
- **Coexistence.** `RadioGuard::enable_coex` turns the client's `CoexAbsent`
  into `Coexisting`; the client publishes its status word
  (`CoexStatusType::from(client)`), and its operation priorities are portable
  `CoexPriority` values that the backend maps onto its events and PTI.
- **Install.** The port's install consumes the membership with the client's
  other owners, so the installed port holds it for its lifetime.

The order is each client's: no proof depends on another client's power.
Wi-Fi prepares the PHY and enables coexistence before it enters power; after
a full common-power cycle it works again only in that order, which the
power-first order breaks on a cold RF restart (). Bluetooth enters power
first and enables coexistence after its PHY engine, as the vendor Controller
does. IEEE 802.15.4 enters power first and takes part in coexistence on
request. Teardown leaves coexistence before power.

| `RadioClient` | Join | Membership | Leave | Coexistence |
| --- | --- | --- | --- | --- |
| `Wifi` | `prepare_phy`, then `power_up_esp32s31_wifi`, then `start_esp32s31_wifi` proves the domain ready and calls `join_wifi` | `WifiPhyMembership`; `suspend_wifi` turns it into `WifiPhySuspended` for modem sleep and `resume_wifi` back with a new `PhyReady` | `leave_wifi`, `leave_suspended_wifi` | `enable_coex` before power, as `wifi_hw_start` enables it right after `esp_phy_enable`; `WifiStopped` carries the power proof and the `Coexisting` as its membership `M` (`with_membership`, `leave_membership`), and only `WifiStopped<W, ()>` releases; status word `Wifi`, `request_wifi_coex` |
| `Bluetooth` | The Controller's `join_phy` calls `join_bluetooth` with the `PhyReady` | `BluetoothPhyMembership`, held by the joined Controller epoch | `leave_bluetooth` | `enable_coex` after the BLE PHY engine, held by `BluetoothSystem`; status word `Ble`, `request_bluetooth_coex`; event priorities from `CoexistenceLevel` (`Baseline`, `Elevated`, `Critical` = `CoexPriority::Normal`, `Elevated`, `Critical`) |
| `I` | `RadioGuard::join_i` runs `prepare_phy`, `phy_ready` and `join_i` | `IPhyMembership` | `RadioGuard::leave_i` | Optional, as ESP-IDF's `esp_coex_wifi_i154_enable`: `enable_i_coex`, status word `I`; `stop` leaves coexistence itself; MAC PTI per scene from `Ieee802154CoexLevel` (`Idle`, `Low`, `Middle`, `High` = `CoexPriority::Idle`, `Normal`, `Elevated`, `Critical`) |

## Limits

Persisting the calibration cache across resets is the caller's policy. Wi-Fi
publishes its coexistence status and reacts to its phases. Bluetooth LE
publishes its status (advertising, scanning, connection) and enables
coexistence for each Controller epoch, but does not react to phases: the
vendor BLE Controller only logs them. IEEE 802.15.4 publishes its enabled
status and follows its own operation scenes, not the phases. There is no modem retention or light
sleep: RF close keeps the registration and calibration, nothing else. A tracking failure
leaves the domain poisoned; the chip must be reset.
