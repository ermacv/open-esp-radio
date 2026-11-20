# Bluetooth LE backend conformance

`oer-protocols-bluetooth-le-testing-conformance`: the shared conformance
suite of the [Bluetooth LE port contract](../../contracts/port/README.md). It
is a testing package: the model port
(`oer-protocols-bluetooth-le-testing-model`) takes it as a dev-dependency, a
chip's port as a test dependency, and no product package depends on it
(`no_product_package_depends_on_a_conformance_suite_or_a_model` of
`oer-tools-test-layers`). It takes the port and the wire formats alone.

A port's test implements `Harness`: its port and capabilities; the
hardware's clock, moved to an instant; what the peer of a role does in its
next event (`Peer`); an event the hardware does not run; the anomalies the
backend can raise; a start that fails; the values the port's calls take;
and what the hardware was asked to program in a run (`record`) and what it
holds (`state`), which the suite compares for equality only. Each property
is one public function, called with the installation's rights (`Rights`).
The suite keeps the rights in its own places and reads no bookkeeping of
the port's.

| Property | What it checks |
| --- | --- |
| `every_right_is_in_exactly_one_place` | A run of every role (each instance an event, a connection's chain a PDU, a reception taken and released) and the stop hand every place back to the suite; a retirement takes them whole. The accept list's right is lent to its write by `&mut` and cannot leave: a type |
| `an_event_ends_once_as_what_happened` | `Executed`; cut, with what was exchanged before the cut; `Late`; `Displaced`; `Withdrawn` by a withdraw and by a stop before it ran; `Indeterminate` by a stop while it ran. A later round gives no end of an event that ended; that an end comes once is also its place's, which is neither `Copy` nor `Clone` |
| `events_end_in_the_airs_order` | Two events, the later published first, end in the air's order |
| `a_withdraw_is_of_one_instance` | A withdraw of one connection ends its listed event `Withdrawn` and its chain `Withdrawn`; the other connection's event and chain stay out and end as they run |
| `the_chain_leaves_in_the_order_submitted` | Three PDUs leave in the order submitted, one in each event whose central acknowledges; an event whose central withholds its acknowledgement, and one whose central is not heard, give no transmit end and leave the chain as it was. A PDU's end comes once: its place is neither `Copy` nor `Clone` |
| `a_received_pdu_is_in_one_place` | Receptions read and not taken, and receptions not read, are back by the close of their round: over twice the chain's nodes, each event's PDU finds a node. A PDU is read in its event's round, taken and held across rounds and read in place; with every node held the peer's PDU is neither taken nor acknowledged and comes again once a release frees a node; one event's receptions come in the air's order; PDUs held across a stop are read in place and released after it. A released node is back by the close of the round of its release |
| `the_indication_holds_until_the_stop` | After each anomaly the facts say the port needs a stop; a round gives no event's end and no transmit end; the scheduler runs nothing; the stop's rounds give everything that ended before and every end of the stop |
| `a_stop_ends_everything` | A stop of a start in progress leaves the state before the start; a stop of a running radio with an event begun (`Indeterminate`), one listed (`Withdrawn`) and a chain (`Withdrawn`), after each anomaly; an alarm armed before the stop raises no fact after it |
| `a_second_run_is_as_the_first` | The record of each run and the state after each stop equal the first run's and the state before it: after every role's run, after each anomaly, after a failed start, after a failed start with an anomaly raised while it was in progress (no fact after it), and eight runs in a row |
| `no_call_returns_an_error` | The port's calls, as the trait declares them, answer outcomes and rights: a type, which a changed signature fails at build |

Property 8 of the contract (the plan of the air) is the controller's. The
suite adds no executor: its polls use a no-op waker, and a port whose
answer is pending is a failure of the property that met it.
