# ESP32-S31 Wi-Fi station facade

`station(resources, config) -> (Control, NetDevice, Runners)` assembles the
S31 lower-MAC backend, arbiter, station/upper-MAC/RSN and portable station
runtime. The separate crate keeps both w3 and HIL wifi-station above that
composition; its explicit layer role is `Facade`.

Control and the affine neutral Ethernet end exist before installation. `Control::wait_started` gates image workload startup on
the completed station construction; cancellation retains the waiter, and losing
an uninstalled runner wakes it with a typed failure. Commands contain only a kind and correlation. The runtime receives the
station's installation-scoped permissions when the P1 runner constructs the
station, retains them through each terminal, and delivers every completion
or refusal directly to the image's concrete callback. No scoped application
callback, port identity or installation loan is required by w3.

The image declares `bind_station!` with its interrupt entries, station
callbacks, collectors, key-unwrap engine and handled-error terminal policy.
`Resources` consumes the unique hardware partition, binding proof, doorbell,
placed DMA allocations, signals and declared storage. `Runners::driver` runs
on the existing P2 executor; `Runners::station` runs on P1 beside networking.
The image supplies the latter's validated open/personal `Profile` and the
selected key-unwrap engine. HIL can obtain credentials after spawning without
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
