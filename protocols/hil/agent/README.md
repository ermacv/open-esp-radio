# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), console serialization and
the lifecycle acknowledgement barrier and the reset-retained post-mortem codec. The
[esp32s31 HIL agent](../../../firmware/hil/esp32s31/README.md) composes these modules with
its executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `capture`, `console`, `lifecycle`, `postmortem` |

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
It is independent of the mounted `lifecycle::Request` reset service. The
image's own `WatchdogRequest` and `UsbRequest` declarations live beside its
hardware actions; USB pad switching and watchdog fault injection are not
agent services.

The optional `lifecycle::Request` service advertises `lifecycle/reset` and
`lifecycle/reset-accepted`. Discovery, key pages, boot evidence and link
health stay in `base`; Hello is also the reachability check. The image queues
a reset request; `lifecycle::acknowledge_reset` publishes the acknowledgement
and returns an affine `AcknowledgedReset` only after its complete frame was
written. The image consumes its correlation observations in its existing
retained reset path. A completion-barrier error returns to the image's
fault policy without performing the requested action. The agent owns no
hardware reset operation.

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

## Capture service

An image's observation collector is a fixed bank of saturating atomic counters
by event kind, including supported zero counts. It implements `oer-trace::Observer`
and retains no event values, clock, record buffer, mask or freeze state.
`Collector::new` rejects duplicate kinds; images declare their bank in a const
initializer. Unlisted events and saturated counters appear as typed failures in
the description. Observation never waits for IO and cannot affect its owner.
Images reset counters at their own operation boundaries and supply capture
identity to `collector.reader(identity)`.

The existing capture service exposes descriptions and bounded counter pages.
A counter reader loads live atomics: pages do not promise one simultaneous cut.
Its fixed bank is immediately ready for pagination (`frozen` in the generic
transport), and it reports zero record pages. The generic record transport also
serves independent PHY experiment data through the radio-health reader; those
payloads and their RF lifecycle belong to that experiment contract. It is not
an event-ring decoder. Pages contain at most sixteen counters, fitting the
existing frame bound. Fault/postmortem records and debug-control are separate
contracts and retain their own lifecycles.
