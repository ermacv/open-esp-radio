# Shared IEEE 802.15.4 MAC values and transactions

`oer-espressif-ieee802154-mac-values` is the semantic vocabulary shared by the
MAC engine, chip HALs, knowledge and host tools. It contains configuration
values and commands, events and abort reasons, RX/TX observations, Multi-PAN
identities, ACK timeouts, diagnostic counters, ETM routes and timer identities.
It also validates 2.4 GHz channels and provider-relative transmit-power level
sets, retaining the channel and level-storage lifetime in each resolved power.

`Ieee802154AckTimeout` represents every sixteen-bit timeout field value.
`from_microseconds` rounds upward in 16 µs units and retains the rejected
request in its error; `from_units` accepts an already bounded field value.
`units` and `effective_microseconds` expose the programmed duration.

The event observation keeps unnamed lanes 7 and 13 in separate named fields.
It gives them no invented meaning and grants no acknowledgement authority.
Any asserted unnamed lane makes classification fail without losing either
observation. A chip without a lane supplies `false` for it.

The crate owns no registers, DMA storage, engine or protocol policy. Chip PACs
own register geometry and raw access. HALs translate the shared semantic values
into their PAC's field values and perform the transactions. Interrupt arming
and deactivation execute through the narrow transition port. The driver's LL
trait and hardware helpers remain in `oer-espressif-ieee802154-engine`.

The engine adapts validated chip power levels into portable MAC capability
types; this crate has no dependency on the MAC protocol or the engine.
DMA frame storage, slot/address bookkeeping, FSM, PIB, coexistence policy and
retained fault reporting are owned above the dictionary.

## License

Apache-2.0. The MAC vocabulary and transmit-power resolution include modified
ESP-IDF code from revision `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`,
`components/ieee802154/driver` and its public low-level vocabulary, ported and
modified by Vladislav Ermachenkov. Derived source files retain the revision,
copyright, license and modification notice; see the root [NOTICE](../../../NOTICE).
Dependent packages declare their effective Apache-2.0 license in Cargo metadata.
