# IEEE 802.15.4 reference model

`oer-ieee802154-model` is an in-memory host backend for the
`oer-ieee802154-port` owner contracts. It uses `no_std` with `alloc`, has
Apache-2.0 licensing and depends on wire/port vocabulary rather than MAC policy.
Its world handle injects received frames, operation completions, receive-window
boundaries, clock observations and faults. These are model observations, not
claims of hardware timing or software cipher implementation.

`OwnedModel::new` returns the backend and its `ModelHandle`. The caller registers
the backend in its local radio slot, then runs a port body with `run_port` and
`LocalExecution`. The body configures two interfaces and takes affine operation
capacity; a terminal releases that capacity before delivery. Joining and
`OwnedModel::resume` retain the same stopped hardware world.

State and permit queues transfer local ownership through `Cell`. Internal
transactions move only plain values and `Rc` owners. Waker clone, drop and wake
callbacks run with all state restored, so callbacks can reenter the world.
Permit waiters use their exclusively borrowed seat identities, without a
wrapping ticket counter. Pending-address snapshots are keyed by slot identity.

External interface and frame-length errors return `ModelInputError` before
mutation, even when disabled or full. Exhausted request IDs poison the backend
with `ModelFault::RequestIdExhausted`, preserving the last ID. Accounting failures
retain their source observations and queued terminal. Loss markers belong only
to the internal queue; public events always have a total portable view.

The explicit `test-support` feature enables the radio port's test support and
the model's `MockCleanup` implementation. Unit tests enable the same dependency
through a dev-dependency. Normal consumers do not receive conformance cleanup
authority. Port-owned UI fixtures remain with `oer-ieee802154-port`.
