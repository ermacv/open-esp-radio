# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), console serialization and
the reset-retained post-mortem record. The
[esp32s31 HIL agent](../../../firmware/hil/esp32s31/README.md) composes these modules with
its executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `console`, `postmortem` |

## Image keys

A request service is a `requests!` declaration: its enum decodes the
endpoints it serves, and its `Service::KEYS` holds each endpoint's key, the key of that
endpoint's response and the keys of the topics and properties in its
`publishes [...]` list. `image_keys!` declares an image's `ImageKeySet` from
the `Base` service (the intake's own module) and the services the image
mounts. The set is sorted, deduplicated (a response two endpoints share
counts once) and checked against the wire bound at compile time; the intake
announces it in `Hello` and pages it out.

An image component with no requests can implement `Service` for its
properties alone. The ESP32-S31 image's `PanicHook` service reports
`system::PanicHook` under the same `panic-hook` configuration as its entry.
It is separate from `PanicRequest`: serving `InjectPanic` does not imply a
hook. `UsbRequest` includes `system::UsbJtagOff` in its published properties.

Terminal completion waits are bounded to sixteen receivers. Exhaustion returns
`ReceiverLimit` with the requested sequence, last published position and capacity.
The image retains this impossible-state fault through its existing fatal record,
keeps the operation's owners quarantined and resets. It never performs the USB,
watchdog, panic-injection or radio-release action after a failed barrier.

`Console::run` returns a `RunFault` owning the endpoint on an invalid read length
or counter-borrow conflict. `link_health` reports counter-borrow conflicts as a
`ConsoleFault`. Checkpoint and rate count overflows return typed
faults before mutation; images retain their observations and reset.

The retained record is a value codec; placement and reset belong to the image.
Const size, alignment and offset checks fix its layout. `AnyBitPattern` proves
validity of arbitrary retained bytes; checkpoint storage uses primitive native
words with explicit byte-preserving conversions instead of a `Pod` byte-cast API.
No retained capacity or allocation changes.

Run the tests:

```console
cargo test -p oer-hil-agent
```

Radio-health page messages and capture storage belong to `oer-esp32s31-instrumentation-hil`; the agent has no radio dictionary dependency.
