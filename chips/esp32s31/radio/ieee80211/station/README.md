# ESP32-S31 Wi-Fi station facade

`oer-chips-esp32s31-radio-ieee80211-station` holds the S31 Wi-Fi parts an
image binds: the binding of its interrupt entries and storage, the
hardware and DMA resources, the station's credentials and settings, the
shared RF actor's resources and the facade's trace observations. Its
explicit layer role is `Facade`; the separate crate keeps HIL wifi-station
above it.

The image declares `bind_station!` with its interrupt entries, station
callbacks, collectors and handled-error terminal policy. The binding names
where the station's network end runs (`Binding::Notifies`): `SameHart`,
local wakers throughout, or, with the macro's `across_harts` arm,
`AcrossHarts<N, A>`, whose notify types raise the executor signals of the
tasks on either side. Every queue end, the link, the network stop and the
lent buffers' returns name their notify types, so a wake across harts is a
direct call and no waker crosses harts.
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
The image supplies the station's start-time `Profile`: the station's address,
scan channels, callbacks, AES engine and event sink.
`StationSettings::open`, `wpa2_passphrase` and `wpa2_pmk` validate the SSID
and credentials without exposing STA or RSN types. `prepare` consumes that
affine owner: open and prederived PMK provisioning complete immediately;
passphrase provisioning yields after each bounded PBKDF2 step. The password
is erased before the first yield, partial derivation is erased on
cancellation, and the prepared PMK (`PreparedCredentials`) is erased on
drop. WPA2 uses PSK/PSK-SHA256 only, including on transition access points;
no password is retained for SAE. Existing core users can still select their
personal policy that upgrades to SAE.

The facade's `Callbacks` and boot-seeded `NonceSource` hide the STA callbacks
and RSN nonce implementation. Stream exhaustion returns `NonceExhausted` for
the image's terminal policy; the stream never restarts at reassociation.

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
the facade's bank uses it directly. The RX block-ack owner retains its
2 KiB compile-time bound.
