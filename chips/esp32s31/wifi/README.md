# ESP32-S31 Wi-Fi station facade

`station(resources, config) -> (Control, NetDevice, Runners)` assembles the
S31 lower-MAC backend, arbiter, station/upper-MAC/RSN and portable station
runtime. The separate crate keeps HIL wifi-station above that
composition; its explicit layer role is `Facade`.

Control and the affine neutral Ethernet end exist before installation. `Control::wait_started` gates image workload startup on
the completed station construction; cancellation retains the waiter, and losing
an uninstalled runner wakes it with a typed failure. Commands contain only a kind and correlation. The runtime receives the
station's installation-scoped permissions when the P1 runner constructs the
station, retains them through each terminal, and delivers every completion
or refusal directly to the image's concrete callback. No scoped application
callback, port identity or installation loan is required by the image.

The image declares `bind_station!` with its interrupt entries, station
callbacks, collectors, key-unwrap engine and handled-error terminal policy.
`Resources` consumes the unique hardware partition, binding proof, doorbell,
placed DMA allocations, signals and declared storage. `Runners::driver` runs
on the existing P2 executor; `Runners::station` runs on P1 beside networking.
The image supplies the latter's validated `Profile` and selected key-unwrap
engine. `StationSettings::open`, `wpa2_passphrase` and `wpa2_pmk` validate the
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
`StationExit` distinguishes stopped, failed, poisoned, quarantined and
unavailable results. Its facade-owned `failure` vocabulary exhaustively
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
The observation vocabulary is `oer-ieee80211-instrumentation`, including
`StationActivity`; the facade exports no observation dictionary. The target
used to correlate raw authentication/beacon observations and all diagnostic
TX classification exist only under `cfg(oer_trace)`.

Typed constructors reject invalid regulatory channels, addresses and SSIDs.
The existing backend terminal capacity has one public canonical constant;
the facade's bank and runtime compile-time capacity check use it directly.
The RX block-ack owner retains its 2 KiB compile-time bound.

Host tests exercise pre-installation control/NetDevice construction, queue
admission, exact TX-owner return and poisoned connection refusal. The
runtime's model tests cover correlated terminals, duplicate in-flight
requests, credit reuse, reconnect epochs, cooperative stop and retirement.
