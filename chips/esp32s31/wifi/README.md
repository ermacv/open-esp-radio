# ESP32-S31 Wi-Fi station facade

`station(resources, config) -> (Control, NetDevice, Runners)` assembles the
S31 lower-MAC backend, arbiter, station/upper-MAC/RSN and portable station
runtime. The separate crate keeps HIL wifi-station above that
composition; its explicit layer role is `Facade`.

Control and the affine neutral Ethernet end exist before installation. `Control::wait_started` gates image workload startup on
the completed station construction; cancellation retains the waiter, and losing
an uninstalled runner wakes it with a typed failure. `connect`, `disconnect`
and `scan` return an `Operation` that resolves once to its request's terminal;
the correlation stays internal. `wait_stopped` returns a copy of the exit
the runner publishes after the radio release, or `RunnerLost` if the runner
ended without one; owners stay with the runner. The runtime receives the
station's installation-scoped permissions when the P1 runner constructs the
station, retains them through each terminal, and delivers every completion
or refusal directly to the image's concrete callback. No scoped application
callback, port identity or installation loan is required by the image.

The image declares `bind_station!` with its interrupt entries, station
callbacks, collectors, the composed engine type and handled-error terminal policy.
`Resources` consumes the unique hardware partition, binding proof, doorbell,
the facade's affine `DmaResources` claim, signals, declared storage and the
value-owned key-unwrap engine supplied by `compose::crypto::key_unwrap`.
The permanent RX ring and nine TX allocations have one declaration in the
facade. Their HAL placement proof stays in the existing HAL macro; image IRQ
modules hold only their line and signals. `Resources` consumes the opaque
`WifiHardware` and DMA owners without exposing their backend field types. `Runners::driver` runs
on the existing P2 executor; `Runners::station` runs on P1 beside networking.
The station runner retains that engine from construction; the image supplies
its validated `Profile`. `StationSettings::open`, `wpa2_passphrase` and `wpa2_pmk` validate the
SSID and credentials without exposing STA or RSN types. `prepare` consumes
that affine owner: open and prederived PMK provisioning complete immediately;
passphrase provisioning yields after each bounded PBKDF2 step. The password
is erased before the first yield, partial derivation is erased on cancellation,
and the prepared PMK is erased on drop. `Profile::new` consumes the prepared
owner. WPA2 uses PSK/PSK-SHA256 only, including on transition access points;
no password is retained for SAE. Existing core users can still select their
personal policy that upgrades to SAE.

The facade's `Callbacks` and boot-seeded `NonceSource` hide the STA callbacks
and RSN nonce implementation. Stream exhaustion returns `NonceExhausted` for
the image's terminal policy; the stream never restarts at reassociation.
`StationExit` distinguishes stopped, failed, poisoned, quarantined,
retirement-failed, release-failed and unavailable results. Its facade-owned `failure` vocabulary exhaustively
converts the backend's nested causes, preserving numeric observations,
tracking classes and radio request identity. It contains no backend type.
Runtime's existing nominal command and status values remain the Control
vocabulary. HIL can obtain credentials after spawning without
moving an installation credit into its command handler.

`RadioResources` constructs the same shared RF actor, calibration identity,
RC-slow membership and cooperative timer. The image retains its SAR/PWDET
interrupt binding and spawns `RadioActor`. The facade has no diagnostic
counters: its copied trace observations reach image-owned collectors, and its
product events are immediate callbacks. Bridge timers, polling, protocol
parameters, DMA geometry and rate-selection policy retain their values.
The facade's own trace events (station activity, the station service's
spans and the selected data rate) are its `observation` module, reported to
`Binding::Observer` and `Binding::DriverObserver`. The protocol's events go to `Binding::Sink`, which
the station service lends its cores; `()` keeps none. The target used to
correlate raw authentication/beacon observations and all diagnostic TX
classification exist only under `cfg(oer_trace)`.

Typed constructors reject invalid regulatory channels, addresses and SSIDs.
The existing backend terminal capacity has one public canonical constant;
the facade's bank and runtime compile-time capacity check use it directly.
The RX block-ack owner retains its 2 KiB compile-time bound.

Host tests exercise pre-installation control/NetDevice construction, queue
admission, exact TX-owner return and poisoned connection refusal. The
runtime's model tests cover correlated terminals, duplicate in-flight
requests, credit reuse, reconnect epochs, cooperative stop and retirement.

## Reconnect and lifecycle

`Config::with_reconnect(ReconnectPolicy::every(interval)?)` enables automatic
initial connection and retries on the existing station runtime timer. A zero
interval is refused. The default is `ReconnectPolicy::DISABLED`. Explicit
Disconnect pauses the policy; explicit Connect resumes it. A connect credit
stays with Runner until refusal or completion, including policy attempts.

`Control::stop` requests graceful shutdown independently of command queue
capacity: disconnect once, wait for link-down, then retire Runner and its
installation owners. Terminal errors do not reconnect or recreate resources.
After an ordinary uninstall the station runner hands `Joined` to the driver
task, which retires the slot, takes the stopped device's Wi-Fi frontier and
membership cell and releases the radio. The station's exit is published only
after that release: a lost handoff, a refused retirement, a still-active MAC,
an unanswered leave or a poisoned domain is `ReleaseFailed`, and an
installation that fail-stopped instead of retiring is `RetirementFailed`;
neither reports `Stopped`. Owners a failed step does not return stay
quarantined in their static backing.
`StationExit` retains the facade's original typed failure. Its public failure
values carry no transport encoding; no HIL dependency, observation counter or
collector is part of the facade.
