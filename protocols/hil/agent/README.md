# HIL agent

`oer-protocols-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), the console and the
image's handlers it calls (`console`, `serve`) and the reset-retained
post-mortem codec. The
[HIL agent on the application entry](../../../hil/agent/README.md)
composes these modules with the library's executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `capture`, `console`, `serve`, `postmortem` |

## Image keys

A request service is a `service!` declaration: its enum decodes the
endpoints it serves, its `Service::KEYS` holds each endpoint's key, the key of
that endpoint's response and the keys of the topics and properties in its
`publishes [...]` list, and its `Service::REQUESTS` the endpoints' keys.
`image_keys!` declares an image's `ImageKeySet` from the `Base` service (the
intake's own module) and the services the image mounts. The set is sorted,
deduplicated (a response two endpoints share counts once) and checked against
the wire bound at compile time; two services that serve the same request do
not compile. The intake announces the set in `Hello` and pages it out.

`service!` declares the kind of each endpoint. An **observation**
(`observe`) is a method of the service's observing trait, called in the
console's step that decoded the request and answered at once. A
**command** (`command`) is a variant of the service's commands enum, run on
the commanding value: with `commander = …`, an `async` method of the
generated commanding trait; without it, the image's own `serve::Run` for
the enum (the pair's probes forward it to their radio's task). An image
implements the traits; a missing or extra method does not compile. A
handler that cannot answer still returns its endpoint's response type, so
every request is answered. Several services compose as `base::Either`, with
a tuple of observing and a tuple of commanding values; nothing is `dyn`.

An image component with no requests can implement `Service` for its
properties alone.

Discovery, key pages, boot evidence and link health stay in `base`; Hello
is also the reachability check.

The console has one holder, its task, and takes no lock and no critical
section. Its state is a `Writer` the image places once and lends to
`console::run`: the encoder, the sequence it numbers messages with as it
writes them and its counts. Nobody publishes into it. It writes Hello
first and answers each base-module request and each observation in the
step that decoded it. Every answer is serialized once, from a reference,
into the encoder's buffer (`serve::Frame::write`).

The image gives `console::run` what observes and what commands as two
values, split once at its composition. A command's future borrows the
commanding value alone, so observations and the base module are answered
while it works; one command is in work at a time by that borrow, and a
second is answered `Rejected(Busy)`. A command's answer is moved once out of
its future and written from where it lies. A command that gives no answer
(`serve::Unanswered`), or an answer that cannot be encoded, is refused to
the host as `Rejected(Internal)` and counted in `tx_dropped`. What follows
a command's write (`serve::After`) travels with its answer: a set that
declares `traps = true` (the fault service) may ask for `After::Trap`, and
`console::run` ends with the trap once the answer is written whole; a set
without it cannot build one. The agent owns no hardware reset operation.

`console::run` ends with a `RunFault` owning the endpoint on an invalid read
length. Checkpoint and rate count overflows return typed faults before
mutation; images retain their observations and reset.

The retained record is a value codec; placement and reset belong to the image.
Const size, alignment and offset checks fix its layout. `AnyBitPattern` proves
validity of arbitrary retained bytes; checkpoint storage uses primitive native
words with explicit byte-preserving conversions instead of a `Pod` byte-cast API.
No retained capacity or allocation changes.

Run the tests:

```console
cargo test -p oer-protocols-hil-agent
```

## Capture service

`capture::Request` advertises `capture/describe`, `capture/records/read`,
`capture/counters/read` and their replies, including the typed
`capture/refused` response. An image mounts the service with a `Reader`;
the agent has no radio dictionary or platform dependency. The image owns
arming, freezing, release, identity allocation, clock and storage. A page
can only read an immutable cut. Boot correlation comes from the HIL envelope;
capture identity, epoch and the full record schema key identify that cut.

`capture::collector::Collector` is caller-owned and implements `oer-portable-trace`'s
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
its RF lifecycle stays in the image. `oer-tools-hil-link::instrumentation` drains
and validates the generic pages before domain decoding.

The trace-only `capture::atomic::AtomicCollector` collects counter-only events
in a producer context separate from the reader. A producer or freeze operation
never waits for an interrupted producer. Refused admission, kind exhaustion
and saturation remain visible. A successful freeze prevents further writes;
`Collector::merge_counters` joins that immutable aggregate cut before the
image freezes its local history. No owner or radio dictionary enters the agent.
