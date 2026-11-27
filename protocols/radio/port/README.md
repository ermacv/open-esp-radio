# Radio port vocabulary

`oer-protocols-radio-port` holds the vocabulary radio ports share: capacity as
values, the result of a port call, the marker of lost events and the radio
clock.

| Value | Meaning |
| --- | --- |
| `Mint`, `branded`, `PoolSet`, `PoolBuilder`, `Pools`, `Slots`, `Slot` | Complete affine capacity issuance: `branded` runs an installation with a fresh brand and one `Mint` of its layout, which issues the whole set once. No slot is `Clone`, `Copy` or `Default` |
| `PoolPath`, `Root<Id>`, `Child<Parent, INDEX>`, `SlotIdentity`, `LocalId` | Sealed component paths of a layout and the identity at their root. A type alone grants no capacity |
| `PortResult`, `Poisoned` | Commands and clock calls return `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one carries the backend's cause, which portable code passes on without interpreting it |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver, so the gap holds only data the protocol does not promise (received frames) and the consumer continues |
| `ClockInfo<D>`, `RadioStamp<D>`, `ClockSample<D>`, `ClockError` | `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` of `oer-portable-time`, which holds the relation of two clock domains and its one projection both ways ([time](../../../docs/time.md)); `ClockError` says why a reading could not be taken |

## Component authority and cost

`Root<Id>` is the path of a standalone layout belonging to identity `Id`. A
tuple adds one `Child` per component at each nesting level; custom factories
preserve the path they receive, including its `PoolPath::Identity`. The
default root is `Root<LocalId<'id>>` for the token's local brand.

`Mint<'p, Layout, Id>` issues `Layout::Pools<'p, Root<Id>>`. The token loan
`'p` and the path's identity are separate invariant parameters: two slots with
the same loan, kind and component index still differ when their identities
differ. Paths and identities occupy no storage and use no registry or claim
latch; a slot index is one byte, and a layout of more than 256 slots fails at
instantiation.

Contract misuses and their positive twins live in `tests/ui/` and the
consumers' corresponding directories: foreign and nested paths, the same kind
twice, a foreign identity and a slot's escape from its loan.
Unit tests cover empty and nested layouts and all 256 indices. The crate
allocates no heap memory and contains no unsafe code.
