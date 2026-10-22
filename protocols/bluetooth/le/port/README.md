# Bluetooth LE port

`oer-bluetooth-le-port` is the portable contract shared by an LE backend and
its reference model. Lifecycle uses `oer-radio-port`: caller-owned `PortSlot`,
P1 `SlotFrontend`, P2 `SlotRegistration`, consuming `Owner` transitions and
supervised `run_port`. No hardware, PHY membership, SRAM addresses or HAL
proofs cross this boundary.

`Request` carries a `LeOperationId`, one affine capacity bundle, an operation
and an opaque `PortContext`. Its operation and completion envelopes currently
contain DTM TX, RX and End. TX supplies a standard payload pattern and length;
the backend generates the bytes. RX requires no transmit-power value. The
backend advertises its exact `LeCapabilities`; `Supported` checks configurations
before submission. DTM runs under an explicit test-air profile and has no
production hard-window admission contract.

Capacity is acquired in FIFO order using an affine waiter seat. Each bundle
reserves one command place, item lease and mandatory terminal place. A bundle
stays reserved through terminal dequeue, so unread terminals cannot be
silently overwritten. Unused capacity can be released once. The DTM session
lease separately spans successful start through End or confirmed withdrawal.
The declared pool layout is `(Permits<CommandCapacity, N, S>,
Slots<DtmSessionKind, 1>)`; paths retain the tuple's distinct component identities.

Admission is synchronous. An inner refusal returns the entire unmodified
request; an outer poison transfers custody to durable backend quarantine.
P1 samples monotonic admission time before enqueue; P2 validates the current
epoch before publication. Caller IDs exclude the top 256 values. A receipt
carries correlation and epoch but grants no release authority.

One owned event stream reports mandatory terminals. `next_event` only dequeues;
dropping its future detaches its waker without cancelling admitted work or
releasing a lease. Executed, NotExecuted, confirmed Aborted and Indeterminate
retain the meanings of `le/radio`. Unknown reachability returns no reusable
session. Disable settles admitted work through its terminal sink; a rejecting
sink returns the whole terminal to quarantine. Normal uninstall requires P2's
stop proof and the shared supervisor's timely Joined authority.

`test-support` exposes allocator-free conformance scenarios, reusable port
transactions and validators of actual returned owners. Hardware cases retain
their classification when an operation is unsupported. Injected loss,
publication holds and faults have explicit ModelOnly reasons. UI twins cover
foreign pools, body epochs, context substitution, repeated release and session
state. Backends keep BodyLoan conversion authority private.
