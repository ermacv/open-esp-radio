# IEEE 802.15.4 OpenThread radio

`oer-i-openthread` implements the `Radio` trait of the
[`openthread`](https://crates.io/crates/openthread) crate over the running
phase of any IEEE 802.15.4 port on the owner shape
([`owner`](../../../protocols/i/src/owner.rs)), so the OpenThread
stack that crate binds can run on any backend of the portable port. It is portable: its dependencies are the protocol, shared radio/time values,
bounded frame storage and the OpenThread radio trait. Host tests cover frame
conversion directly and require no executor or concrete backend. The ESP32-S31 composition's runtime
(`oer-espressif-i-runtime`) is the port the Thread example and the
HIL target use; the ESP-IDF behaviour described below is that of this
adapter over that runtime.

It depends on `openthread-radio`, the trait crate of the repository's fork
of that crate,
[`ermacv/openthread`](https://github.com/ermacv/openthread/tree/oer/main)
at a pinned revision of its working branch `oer/main`, which merges upstream
`esp-rs/openthread` `main` every one to two weeks, keeps every pinned
revision under a `pin/<sha>` tag and records what each merge took in
[`UPSTREAM.md`](https://github.com/ermacv/openthread/blob/oer/main/UPSTREAM.md);
the fork's `openthread` crate at the same revision re-exports the
trait for the application. The fork extends the trait with what OpenThread hands a radio that
claims `OT_RADIO_CAPS_TRANSMIT_SEC` and reads back from it: the MAC keys and
frame counter (`set_mac_keys`, `set_mac_frame_counter`), each frame's
transmit information (`transmit_frame`: `mIsARetx`, `mIsSecurityProcessed`,
and the frame counter and key index the radio wrote), the acknowledgement
sent for a received frame (`PsduMeta::ack`), the capabilities declared
before the instance is built (`OtResources::set_radio_caps`) and, with its
`csl` feature, Coordinated Sampled Listening: the radio clock
(`Radio::clock`), scheduled receive windows (`Radio::receive_at`), the CSL
receiver state (`Radio::set_csl`), delayed transmission (`TxFrame::tx_at`)
and receive SFD times (`PsduMeta::timestamp`), and, with its
`link-metrics-subject` feature, the enhanced-ACK probing initiators of a
Link Metrics subject (`Radio::set_enh_ack_probing`), and, with its
`time-sync` feature, the Time IE of each frame (`TxFrame::time_sync`).

## Use

Start the IEEE 802.15.4 composition and run its backend under `run_port`.
The port's body configures interface 0, enables the radio and lends its
running phase to `OpenThreadRadio::new`: the events consumer, the control,
the client of interface 0 with its operation seat, and the interface's
pending-table slots (`PendingSlots`), which `into_parts` gives back. Pass
`MonotonicRadioClock::new(clock)` over the monotonic clock the port counts
in (the Espressif runtime's `IRuntime::clock`) and
`PortRssi::new(reader)` over the install's recent-RSSI reader to the
`OpenThread` constructor, which reads the radio time (`otPlatRadioGetNow`,
`otPlatTimeGet`) and the live RSSI (`otPlatRadioGetRssi`) through them.
`init` refuses a port without a transmit-power, CCA threshold or CSL
capability, installs zeroed MAC keys and an enhanced-ACK generator, and
tells OpenThread the port's figures (`Ieee802154Capabilities`): the
transmit power and CCA threshold the radio starts with (`reset_dbm`) and
its receive sensitivity, from which enhanced ACKs also measure link
margins. OpenThread's frame power is an upper bound: above the port's
levels the frame goes out at the highest level, below them the
transmission fails. A CCA threshold outside the port's range fails the
operation. A scheduled receive (`receive_at`) is the interface's window
wish, admitted at once. Declare `OPEN_THREAD_RADIO_CAPABILITIES` with
`OtResources::set_radio_caps` before building the OpenThread instance:
OpenThread's `SubMac` reads the capabilities once, when the instance is
built, and leaves transmit security to the radio only when it saw it there.
When the port reports `EventsLost`, only received frames are in the gap:
the adapter reports them to OpenThread as one `RxFailed` reception. A
poisoned port fails every later operation. The application still supplies
what the `openthread` crate asks of a platform: entropy, settings storage
and the C library functions OpenThread links.

After an OpenThread role change, `frames::role_txrx_priority` gives the
coexistence priority of immediate transmission and reception ESP-IDF's
`handle_ot_role_change` sets (low with the receiver on when idle, middle for
a sleepy device); the composition maps it to its radio's coexistence levels,
as the Thread example does with `ISystem::update_coexistence`.

## What the radio reports

The capabilities are the ones the radio keeps under this trait:

- PHY: ACK timeout, energy scan, transmission from sleep, transmit
  security and timed transmission and reception, as ESP-IDF's OpenThread
  port reports them (`otPlatRadioGetCaps`). The CSL accuracy and
  uncertainty are the radio port's `CslCapability`; the Espressif runtime
  publishes ESP-IDF's defaults, 50 ppm and 500 microseconds.
- MAC: hardware acknowledgement in both directions, PAN ID, short and
  extended address filtering, promiscuous mode and source matching. A
  disabled source-match table answers every poll with frame pending; an
  enabled one uses the enhanced pending mode ESP-IDF's port selects for
  Thread 1.2 and later. Table changes reach the radio entry by entry, and a
  full table is no error, as in the port.

As over ESP-IDF's radio, OpenThread's `SubMac` runs CSMA-CA backoffs and
retries itself; each of its attempts reaches the radio as one transmission,
with a CCA when OpenThread asks for one, so the radio's own CSMA-CA and
retries stay unused. The CCA uses OpenThread's energy threshold: the radio
starts from the reported default and takes each threshold
`otPlatRadioSetCcaEnergyDetectThreshold` sets before the next CCA, as the
port hands it to `esp_i_set_cca_threshold`. The radio secures each attempt
as the port's `otPlatRadioTransmit` does: a new frame counter and the
current key index for a first transmission, the frame's own for
`SubMac`'s retransmission, nothing for a frame OpenThread secured itself.
The frame counter and key index it wrote go back into OpenThread's frame,
where `SubMac` reads the used counter. 2015 frames get enhanced ACKs,
secured ones with the same keys and counter; the received frame reports the
ACK's counter and key index and its frame-pending bit, as the port's
receive information does. Unlike the port, which reports the counter after
the one the ACK carried, the radio reports the one the ACK carried. The
radio acknowledges no second short address.

`otPlatRadioGetRssi` reads the install's recent-RSSI reader, which reads
`esp_i_get_recent_rssi`: the low byte of the shared baseband's
current receive information, of the most recent reception whichever
protocol received it; OpenThread's invalid RSSI while no radio is
installed.

The radio clock is the port's `clock` (over the runtime, ESP-HAL's
microsecond clock, where ESP-IDF's port reads `esp_timer`); OpenThread's 32-bit radio times are taken as the nearest
instant of it. A CSL receiver's `otPlatRadioReceiveAt` becomes a scheduled
receive window, outside which the radio sleeps; the first frame of the
window ends it, as in the vendor driver. `otPlatRadioEnableCsl` and
`otPlatRadioUpdateCslSampleTime` set the port's CSL state (`RadioSetting::Csl`): enhanced ACKs
then carry a CSL IE, every CSL IE the radio sends gets the period and the
phase to the next sample time when its SFD goes out, and retransmissions
take a new frame counter, all as the port does. A CSL transmitter's delayed
frame becomes a scheduled transmission, with a CCA when OpenThread asks for
one.

A Link Metrics subject's probing initiators
(`otPlatRadioConfigureEnhAckProbing`) reach the port's enhanced-ACK
generator (`RadioSetting::EnhancedAckProbing`), whose ACKs to an initiator carry the Thread enhanced-ACK
probing IE with the frame's metrics, as the port's generator adds
`otLinkMetricsEnhAckGenData`. Link margins are measured from the receive
sensitivity the radio reports; ESP-IDF's port never sets the noise floor
of its `link_metrics.cpp` utility, so its margins are zero.

With Thread network time synchronization, a frame's Time IE gets the time
sync sequence and the radio clock plus OpenThread's network time offset
when its SFD goes out, as the port's `ot_radio_transmit_sfd_done` writes
them; OpenThread reads the same clock (`otPlatTimeGet`).

Frames that arrive during a transmission or energy scan wait in a bounded
queue for `receive`; a full queue drops the newest. A transmission whose
future OpenThread drops finishes in the backend, and the next operation waits
for its end.

## Build

The adapter needs no OpenThread C library: `openthread-radio` holds the
trait alone, so the adapter builds and is tested on the host, where its tests
drive the trait as OpenThread does over the Espressif runtime and the
engine's register model, and over a host model of the port that has no
runtime, engine or chip behind it. The application's `openthread` crate compiles OpenThread from source
for the chip target through `openthread-sys`, which needs system-installed
CMake, Clang and libclang; host builds of the workspace do not compile it.

## Limits

The HIL cell `i-thread-exchange` attaches an end device over the
adapter to a network ESP-IDF's OpenThread leads and exchanges UDP both ways;
CSL, time synchronization and secured enhanced ACKs are not exercised on air.
The
[Thread example](../../../../examples/esp32s31/thread/README.md) runs an
OpenThread end device over it.
