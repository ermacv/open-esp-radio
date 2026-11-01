# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), console serialization and
the lifecycle acknowledgement barrier and the reset-retained post-mortem codec. The
[HIL agent on the application entry](../../../hil-scenarios/hil-agent/espressif/README.md)
composes these modules with the library's executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `capture`, `console`, `lifecycle`, `postmortem` |

## Image keys

A request service is a `requests!` declaration: its enum decodes the
endpoints it serves, its `Service::KEYS` holds each endpoint's key, the key of
that endpoint's response and the keys of the topics and properties in its
`publishes [...]` list, and its `Service::REQUESTS` the endpoints' keys.
`image_keys!` declares an image's `ImageKeySet` from the `Base` service (the
intake's own module) and the services the image mounts. The set is sorted,
deduplicated (a response two endpoints share counts once) and checked against
the wire bound at compile time; two services that serve the same request do
not compile. The intake announces the set in `Hello` and pages it out.

`service!` declares a service whose every endpoint has a handler: the
`requests!` enum and keys, a handler trait with one required method per
endpoint and `serve`, which awaits the handler of a request and publishes its
response with the request's identity. An image implements the trait; a
missing or extra method does not compile. A handler that cannot answer still
returns its endpoint's response type, so every request is answered.

An image component with no requests can implement `Service` for its
properties alone.

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
keeps the operation's owners quarantined and resets. It never performs the
requested action after a failed barrier.

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

`capture::Request` advertises `capture/describe`, `capture/records/read`,
`capture/counters/read` and their replies, including the typed
`capture/refused` response. An image mounts the service with a `Reader`;
the agent has no radio dictionary or platform dependency. The image owns
arming, freezing, release, identity allocation, clock and storage. A page
can only read an immutable cut. Boot correlation comes from the HIL envelope;
capture identity, epoch and the full record schema key identify that cut.

`capture::collector::Collector` is caller-owned and implements `oer-trace`'s
`Observer`. It is not `Sync` and belongs to one image-selected context.
Observation points remain `observe!`, so a plain image evaluates no event
arguments. The collector counts each event kind, even with its history
channel disabled or history full. Its bounded bank lists observed kinds;
an absent kind is not an advertised supported zero. Full history, bank
exhaustion, saturation and nested access latch typed capture failures,
without returning into or changing the observed operation. The host may
drain incomplete history and still read its independent counts. No capture
state survives reset.

Pages hold two opaque postcard records of at most 128 bytes, or sixteen
aggregate counters. Their maximum encodings fit the existing 560-byte
payload bound. Dictionaries own record schemas and interpretation. The
ESP32-S31 radio-health binding implements `Reader` over its native storage;
its RF lifecycle stays in the image. `oer-hil-link::instrumentation` drains
and validates the generic pages before domain decoding.

The trace-only `capture::atomic::AtomicCollector` collects counter-only events
in a producer context separate from the reader. A producer or freeze operation
never waits for an interrupted producer. Refused admission, kind exhaustion
and saturation remain visible. A successful freeze prevents further writes;
`Collector::merge_counters` joins that immutable aggregate cut before the
image freezes its local history. No owner or radio dictionary enters the agent.
