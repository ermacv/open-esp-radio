# Radio service supervisor

`oer-radio-supervisor` runs the portable [radio service](../../../radio/README.md):
it transports commands through a one-slot mailbox and drives the complete
local role epoch. It is executor-independent: it uses `embassy-sync`
primitives over a caller-chosen raw mutex and returns futures any executor
polls; the composition binds it to an executor.
A controlled child must return its exact owner before stop completion; failure
retains the faulted owner.

The crate root reexports declarations from private message, transport,
stopped-dispatch, active-role and actor modules. The one-slot mailbox carries
bounded value requests and reports, never physical owners. A caller cancelled
after publication does not cancel the owner actor: the port consumes that stale
completion before publishing another command. If the supervisor disappears
during this reconciliation, the next unpublished request is returned as a typed
rejection; disappearance after publication is reported as a faulted operation.
The actor retains stopped, active-faulted and lifecycle-faulted owners
separately, and acknowledges cooperative Stop only after classifying the
returned owner frontier.

The service crate does not depend on this runtime or on any executor. Chip
compositions use both crates; applications reach these types through the
[public facade](../../../oer/README.md) as `oer::runtime::radio`.

Tests use the synthetic portable service profiles that `oer-radio` exposes
with its `test-support` feature; they do not depend on a concrete chip profile.
