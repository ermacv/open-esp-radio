# ESP32-S31 Wi-Fi product integration

`oer-esp32s31-ieee80211-system` composes the public radio lifecycle,
static resources, IRQ bindings and selected network adapter. Applications own
board identity, credentials, IP configuration and sockets. The
[network implementation guide](../../../../../docs/network-implementations.md)
explains the external crates, reasons for patches and complete build commands.

## Start here

Applications bring the radio up once with
[`oer-esp32s31-radio-system`](../radio/src/lib.rs)'s `start(spawner,
EspHalRadioPlatform, RadioStart::new())`: it returns the shared radio in
static storage and its `ConcurrentPartitions`, and spawns the radio's periodic
PHY tracking and coexistence schedule, which every radio client needs. It then calls
the crate's `new` entry point once with the shared radio, the Wi-Fi partition
and the `EspHalWifiPlatform` (the `WIFI` singleton), and receives
`WifiStarted`: the Wi-Fi application capabilities, the bring-up evidence and
the sole `SystemRunner`. Splitting the `WifiSystem` yields the hardware-free
`WifiControl`, network devices, monitor stream and status observers. The
runner, not `WifiControl`, retains the stopped MAC owner, DMA arenas and IRQ
route while it is spawned. Internal crates do not depend on the `oer` facade;
the facade reexports their application-facing contracts.

`RadioConfig::from_efuse(watchdog, initial_channel)` takes the station and
access-point addresses from the chip's eFuse and fails with `EfuseMacError`
when one is not a unicast address; `RadioConfig::new` takes them explicitly.
The watchdog budgets and the initial channel are always the caller's.

This crate reexports every input of `new`, `RadioConfig` and `WatchdogConfig`
and of the shared radio they join: `RadioHardware`, `ConcurrentPartitions`,
`WifiPartition`, `SharedRadio`, `EspHalRadioPlatform`, `EspHalRadioClocks`,
`EspHalWifiPlatform`, `RtsLengthThreshold`, `DeadlineWatchdog`,
`DeadlineBudget`, `EfuseMacError` and the `await_stack_boundary!` poll
boundary. The PHY
calibration identity and an optional retained calibration cache are inputs of
the shared radio, not of Wi-Fi. Board startup and the executor remain
application-owned.

Use the buildable [station](../../../../../examples/esp32s31/station/),
[access-point](../../../../../examples/esp32s31/access-point/) and
[monitor](../../../../../examples/esp32s31/monitor/) applications as the
copyable entry points. They are compiled for the repository's pinned
ESP32-S31 target and exact Cargo feature profiles by the documentation gate.

## Wi-Fi lifecycle

The normal station path crosses these distinct ownership boundaries:

| Boundary | Owner and meaning |
| --- | --- |
| Bring-up on the shared radio | Under one arbiter lease, the first client registers the shared PHY domain (or wakes closed RF), then Wi-Fi joins it, initializes the MAC and constructs the driver's `WifiStopped`. `WifiInitialization::phy` reports what the PHY preparation did. No application role is active. |
| Request planning | `oer-radio` validates the station request while the actor still holds `WifiStopped`. A rejected request is returned with `WifiControl`; no PAC, DMA or IRQ owner moved. |
| Role materialization | The concrete runner consumes `WifiStopped`. The station epoch takes the register owner and inactive IRQ token, installs its DMA/task graph, and acknowledges start only after that graph is owned. |
| Connected data | Network adapters lend packet storage; the chip datapath owns DMA publication and terminal return. A control response does not release a frame or descriptor. |
| Channel changes | Scan, join, AP start, monitor hopping and ESP-NOW off-channel work take the arbiter lease for each channel transaction, so another radio client may use the shared domain between them. |
| PHY maintenance | The shared radio's periodic tracking is the only maintenance; Wi-Fi roles never pause for it. |
| Stop and restart | Stop must recover the exact DMA/task owner and IRQ setup token. Only their reunion with the logical owner reconstructs `WifiStopped`; the next role may then be planned. |

The ordinary TX owner carries the existing `ReceptionTimer`: its monotonic
clock times waits and watchdogs, and its MAC clock supplies generation-bound
radio samples for A-MPDU lifetime decisions. TX requires only the runtime's
read-only `MacClockReader`; RF wake notification and receive timestamp
conversion remain with their existing owners. A new radio start rebinds this
capability only after the physical lifecycle returned an idle descriptor;
reconnection retains the same radio epoch. No monotonic reading is relabelled
as a radio instant.

AP uses the same single physical owner but different role policy and queues.
Same-channel STA+AP is one combined role epoch, not two independently
restartable radios.

`WifiIdle::restart_radio` takes Wi-Fi off the shared radio and brings it up
again while no role is active: it stops the MAC, halts RX and withdraws the
IRQ route, releases the Wi-Fi client (`esp_phy_disable`), closes RF when Wi-Fi
was the last PHY client, and rejoins. The domain stays registered, so the
rejoin wakes closed RF without calibration. `WifiRadioRestartReport::rf`
reports whether RF was closed and woken or kept open by another client.

### Failure and cancellation

- Dropping an application wait does not cancel a command already published to
  the supervisor. The actor continues, and its mailbox drains the stale reply
  before any later internal request. The consumed public typestate is not
  recreated by dropping its future.
- A planning rejection occurs before materialization and returns the original
  request and idle control capability. Errors after owner movement retain the
  physical frontier, either in `Faulted` or through terminal system escalation;
  neither returns a restartable role.
- A fault that ends the station role, alone or beside the access point and in
  any phase (start, the connected epoch, teardown after a stop), reaches the
  application on its link stream: `StationStatus` publishes the terminal
  `StationLinkState::Faulted` with the portable `StaFaultReason` of
  `oer-ieee80211-sta`. Every faulted role epoch passes the one classification,
  `ProductionWifiFault::station_fault_reason`; the connected faults are
  classified by `ConnectedStationFault`'s `StaFault`. Later Wi-Fi commands
  still fail with `RadioError::HardwareFault`.
- The composition never drops a polled station task: the active-role driver
  (`drive_wifi_active_role_pinned`) borrows the pinned role future
  until it returns, and a stop command only asks the role to stop. A
  cancelled `StationTask` is therefore not a station fault and has no
  `StaFaultReason`; its fail-closed `Drop` only guards against misuse.
- Lifecycle failures request system reset for an unconfirmed MAC/RX stop, an
  ambiguous RF close or wake, a started PHY registration that failed, a
  partially completed Wi-Fi release and failed initial tracking or channel
  execution. The composition borrows the retained failure until the SoC
  adapter's diverging reset request; it does not await an application response
  or release DMA/IRQ owners first. A rejected quiesce, a rejected PHY
  preparation, an active MAC at release and a recoverable RF close retain their
  exact non-runnable lifecycle owners without reset. A failed bring-up is
  retained in the lifecycle fault slot rather than dropped when reporting
  `NewError`.
- Radio drivers do not own watchdog/reset peripherals. `RadioConfig` requires
  caller-owned static `WatchdogConfig` storage binding the SoC TIMG1 service
  and explicit startup and shutdown budgets. A lease covers the full physical
  operation; cancellation cannot disable it. No qualified defaults or measured
  RF-stop time bound are supplied. Bring-up arms before the arbiter lease is
  taken and completes at its ready owner. Radio restart uses a shutdown lease
  through quiescence, release and RF close and a separate startup lease
  through the rejoin.
- Once polled, bring-up, release and RF close must reach a terminal result;
  dropping them can strand a partially changed shared radio.

The item-level contracts are in `oer-radio-supervisor`,
`oer-esp32s31-ieee80211::runtime`, and the concrete role modules in this crate's
source. `FEATURES.md` is navigation for scope and limitations; the generated
qualification view remains the readiness authority.

The network stack is the owned Embassy/Xarxa contract with explicit packet
pools, selected by the default `owned-network` feature; it is the only network
implementation.

The [packet ownership contract](../../../../../docs/wifi-egress.md) defines
adapter and physical scheduler boundaries. Static dimensions and the one-time
resource claim belong to this crate; reusable adapters supply storage types.

Multi-kilobyte connected owners live in static storage, so the connected
epoch's futures and teardown frames move pointers to them, never their bytes:
the RX protocol runtime, and the connected control, placed for each
association in a slot in the PSRAM tier
(`.psram.bss.open_radio_station_connected_control`) and released back to the
supervisor by teardown. Interim `size_of` guards in `supervisor/station.rs`
bound the connected faults, exits, teardown failure and the connected
epoch's future until the static stack bound () checks the frames.
A library feature does not establish that every application role has been
qualified.

Applications that construct their own stack can consume `WifiDevice`
through `into_owned()`. The transfer returns its matching packet allocator
alongside the unique device. This permits application-owned stack composition
and observation without exposing hardware authority or cloning an endpoint.
