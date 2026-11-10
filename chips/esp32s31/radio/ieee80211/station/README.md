# ESP32-S31 Wi-Fi station facade

`station(resources, config) -> (Control, NetDevice, Runners)` assembles the
S31 data port (`Esp32s31Port`), the portable runtime's datapath actor over
it and the station's lifecycle with its arbiter and station/upper-MAC/RSN. The separate crate keeps HIL wifi-station above that
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
callbacks, collectors and handled-error terminal policy.
The binding names where the station's network end runs (`Binding::Notifies`):
`SameHart`, local wakers throughout, or, with the macro's `across_harts`
arm, `AcrossHarts<N, A>`, whose two notify types raise the network task's
and the datapath actor's executor signals. The actor polls every station
end of the network's channels, the network's stop answer and the lent
receive buffers' returns; the station's lifecycle shares its executor and
hears it only in same-executor queues. Every queue end, the link, the
network stop and the lent buffers' returns name their notify types, so a
wake across harts is a direct call and no waker crosses harts.
`Resources` consumes the unique hardware partition, binding proof, doorbell,
the facade's affine `DmaResources` claim, signals and declared storage. The
station unwraps EAPOL-Key data (RFC 3394) synchronously in its step over the
engine the facade's station callbacks supply (`StaCallbacks::AesEngine`), on
which its EAPOL-Key MIC and protected-management CCMP and BIP run as well:
the AES accelerator (`runtime::crypto::AesEngine`), each block waited for on
its `STATE` flag, which the image hands to the station's `Profile`.
The permanent RX ring and nine TX allocations have one declaration in the
facade. Their HAL placement proof stays in the existing HAL macro; image IRQ
modules hold only their line and signals. `Resources` consumes the opaque
`WifiHardware` and DMA owners without exposing their backend field types.
`Runners::datapath` and `Runners::station` run on one thread-mode executor
(`DriverExecutor`), so neither preempts the other. The datapath task installs
the S31 data port on the declaration's first minted installation
(`declare_installations!`), places the datapath actor (`Actor`) in the
facade's storage and runs it: the actor owns the radio and carries the data
both ways. The station task runs the station's lifecycle (`Lifecycle`),
placed in the facade's storage, which commands the actor in messages. The
actor's task hands each installation's command issuer and tokens to the
station's task through a one-slot mailbox: at the first install and after
every reinstall. The actor's signals name its executor; lent-buffer returns
on its own hart raise them as its own task (`FromOwnTask`), and the radio
doorbell rings from the executor's wake entry.
The image supplies the station's start-time `Profile`: the station's address,
scan channels, callbacks, AES engine and event sink; the actor takes the
rate selection's observer and its own copy of the sink.
The network comes with each connect: `Control::connect(credentials)` takes
the `PreparedCredentials` of one network, which only an idle station takes
and keeps for its reconnections; a refused, stopped or cancelled connect
drops them, erasing the PMK. `StationSettings::open`, `wpa2_passphrase` and `wpa2_pmk` validate the
SSID and credentials without exposing STA or RSN types. `prepare` consumes
that affine owner: open and prederived PMK provisioning complete immediately;
passphrase provisioning yields after each bounded PBKDF2 step. The password
is erased before the first yield, partial derivation is erased on cancellation,
and the prepared PMK is erased on drop. `connect` consumes the prepared
owner. WPA2 uses PSK/PSK-SHA256 only, including on transition access points;
no password is retained for SAE. Existing core users can still select their
personal policy that upgrades to SAE.

The facade's `Callbacks` and boot-seeded `NonceSource` hide the STA callbacks
and RSN nonce implementation. Stream exhaustion returns `NonceExhausted` for
the image's terminal policy; the stream never restarts at reassociation.
`StationExit` distinguishes stopped, failed (`LifecycleFailure`: a refused
first start, an unconfirmed stop or retune, the port's named fault, the
actor's or the lifecycle's broken contract, a refused reinstall),
release-failed and unavailable results. Its facade-owned `failure` vocabulary
exhaustively converts the backend's nested causes, preserving numeric
observations, tracking classes and radio request identity. It contains no
backend type.
Runtime's existing nominal command and status values remain the Control
vocabulary. HIL can obtain credentials after spawning without
moving an installation credit into its command handler.

`RadioResources` constructs the same shared RF actor, calibration identity,
RC-slow membership and cooperative timer. The image retains its SAR/PWDET
interrupt binding, gives the actor the observer of its periodic tracking
ticks (`observation::TrackingTick`) and spawns `RadioActor`. The actor's
run ends only when its periodic PHY tracking fails: it then keeps every
owner it holds and returns the failure's class and variant path
(`oer_chips_esp32s31_platform_runtime::radio_fault::Failure`) for the image to record and
reset with. The facade has no diagnostic
counters: its copied trace observations reach image-owned collectors, and its
product events are immediate callbacks. Bridge timers, polling, protocol
parameters, DMA geometry and rate-selection policy retain their values.
The facade's own trace events (station activity, the selected data rate, each received data MPDU's rate and A-MPDU
containment and each single data transmission's last rate, transmissions
and end) are its `observation` module, reported to
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
retries on the existing station runtime timer, after an image's connect
linked the station to its network; the policy never connects first, and an
image's connect that ends without a link leaves the station idle, its
retries the image's own. A zero interval is refused. The default is
`ReconnectPolicy::DISABLED`. Explicit Disconnect pauses the policy; an
explicit Connect that links resumes it, one that is refused leaves it as
it was. A connect credit
stays with Runner until refusal or completion, including policy attempts.

`Control::stop` requests graceful shutdown independently of command queue
capacity: disconnect once, wait for link-down, then stop the radio and
retire the actor's installation. A failed station never ends the task: its
lifecycle stops the radio and ends for a restart, the actor's task
reinstalls the port in place on the next installation (the network's lent
units stay counted and rejoin the new ring), and the station's task builds
the station and its arbiter again on the new tokens, with the
configuration and callbacks the retired station gives back; no reset
follows. A refused reinstall is `Failed(Reinstall)`. Before a reinstall or
a release the actor's task runs the stopped port's cleanup once
(`DataPort::clean_stopped`); one that did not finish, a group key entry
the hardware still holds among them, keeps the port stopped with its
owners and is `Reinstall(Cleanup)` or `ReleaseFailed(Cleanup)`, and
nothing runs it again. The cleanup's sink takes only the removal receipt:
a completion the driver still kept, which the cleanup brings home, is
refused there and stays in the port's store, so the port does not retire
and the release ends `ReleaseFailed(NotStopped)` with the port unfinished.
After an ordinary stop the actor's task takes the stopped device's Wi-Fi
frontier and membership cell and releases the radio. The station's exit is
published only after that release, or once the stop's drain passed first:
a cleanup that did not finish, a still-active MAC, an unanswered leave, a
poisoned domain or the passed drain is `ReleaseFailed`, never `Stopped`.

## How an installation ends

A Wi-Fi installation, the station's or the access point's, ends in one of
two ways:

- a **completed retire**: the radio's stop is confirmed, the stopped port's
  cleanup ran, the radio is released, and every owner (the actor or runner,
  its port, the cleanup's removal receipt) ends with the retired
  installation;
- an **unfinished installation** (`unfinished::Unfinished`), returned whole
  to the external owner, the application: the actor or runner with its port
  (`owner`), the items a port call could neither give it nor keep
  (`unkept`), an attempt a submission returned unaccepted (`unaccepted`),
  the cleanup's removal receipt (`receipt`), the cause and the phase
  (`unfinished::Phase`: `Running`, `Stopping`, `Stopped`, `Reinstalling`,
  `Releasing`). Nothing of it is dropped on the way.

A port that refuses its installation installs nothing and hands back
every input it was given (`Esp32s31Port::install`'s `InstallRefused`: the
parts, the minted installation, the driver's place): the station's
datapath task returns them as `DatapathEnd::Refused(RefusedInstall<W>)`
and the access point's as `AccessPointEnd::Refused`, after the started
radio left from the device. A stage before it that refused (no first
installation minted, the radio's start refused, an access point's
settings) returns the registry and the minted installation it had taken
(`DatapathEnd::NotInstalled`, `AccessPointEnd::NotInstalled`:
`NotInstalledOf`); a started radio leaves first, as before.

The station's installation has two halves, and an unfinished one returns
both. The datapath task returns `DatapathEnd` (`Retired`, `NotInstalled`,
`Refused`, or `Unfinished(UnfinishedDatapath<W>)`: the installation
(`UnfinishedStationOf<W>`, cause `DatapathCause`), the actor's ends of the
queues to the lifecycle (`ActorQueues`) with every owner in transit in
them, and the installation registry): a fault of the actor or the
lifecycle's end without retiring it (`Lifecycle`: a stop or retune that
proved nothing, a broken contract), each in the phase the actor's radio
read (`Actor::radio_phase`: a stop asked for and not confirmed is
`Stopping`), a refused cleanup or reinstall after a restart, or TSF epochs
used up (`Reinstalling`), an item the actor could not keep at its retire
(`Stopped`, no release), a refused release (`Releasing`). The station's
task returns `StationEnd`: its `StationExit` and, unless the radio's
release or the restart's reinstall completed, its lifecycle's half
(`UnfinishedLifecycle`: the retired station, places, arbiter, issuer and
contexts, the lifecycle's ends of the queues (`LifecycleQueues`) with
every owner in transit in them, and the actor task's news after the
lifecycle stopped waiting). A lifecycle that ends without retiring the
actor tells the actor's task once and waits until its run stopped
(`News::Abandoned`); only then do its ends go out, so no producer runs
while either half collects its queues, and each owner in transit is in its
queue once, in order. The half's operations read its queues (`queues`),
its events' sink (`events`) and the actor task's late end (`late_end`:
`LateEnd`, kept once read; an out-of-order installation's commands and
tokens through `take_installed`), and finish it once the datapath half
retired (`finish`, on `DatapathRetired`, which only a completed release
makes): the retired lifecycle's owners end, and the late end, the queues
with every owner still in transit and any kept installation come back
(`FinishedLifecycle`). A release that completed after the stop's drain
passed keeps both actual outcomes (the lifecycle's exit and the late end)
and the half ends without a second release (the application's
`StationInstallationEnd::finish_late_retire`). A fault the actor would
recover from after the lifecycle's retire or reinstall ends the actor
instead (no recovery is told to a lifecycle that ended): its news ends the
lifecycle's wait, both halves go out with the fault and the phase the
radio read, and nothing asks for the retire or the reinstall again. That
half stays in the place its lifecycle ran in until the actor's task told
how the end settled; only a completed release or reinstall ends it and
frees the place for the next installation, and any other end moves it
whole to its own home (`Placed::move_to`), so the place is never reused
while the half is in it. The news carries causes, never owners.

The access point's runner returns `AccessPointEnd::Ended(ApExit)` after a
completed retire (or a run that never installed), or
`AccessPointEnd::Unfinished`. An item a port call could not keep goes, before
each watch of the stop and after each watch that returned with one
(`StopWatch::Unkept`), to a free place of the retire's home
(`UNKEPT_ITEMS`), so the stop's watch goes on serving the port; a further
item that finds the home full stays with the runner and ends the retire in
`Stopping`. With the stop confirmed and items out no release runs
(`Stopped`); a refusal the release does not repeat ends in `Releasing`.

The application's `wifi::Station::installation_end` (both halves, once
both tasks ended) and `wifi::AccessPoint::installation_end` hand that value
over. No end of an installation resets the chip: every `StationExit` keeps
its typed cause with the control.

Owners a failed step does not return stay quarantined in their static
backing.
`StationExit` retains the facade's original typed failure. Its public failure
values carry no transport encoding; no HIL dependency, observation counter or
collector is part of the facade.
