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
| `every_right_is_in_exactly_one_place` | A run of every role (each instance an event, a connection's chain a PDU, a reception taken and released), the accept list's write and the stop hand every right back; a retirement takes them whole |
| `an_event_ends_once_as_what_happened` | `Executed`, cut with what was exchanged before the cut, `Late`, `Displaced`, `Withdrawn` by a withdraw and by a stop before it ran, `Indeterminate` by a stop while it ran; no end comes twice |
| `events_end_in_the_airs_order` | Two events published later one first end in the air's order |
| `a_withdraw_is_of_one_instance` | A withdraw of one connection ends its listed event and its chain; the other's stay out and end as they run |
| `the_chain_leaves_in_the_order_submitted` | Three PDUs leave in the order submitted, each once, only in an event whose peer acknowledged; a withheld acknowledgement and a lost packet end nothing |
| `a_received_pdu_is_in_one_place` | A PDU is read in its event's round, taken and held across rounds and read in place; with no node free the peer's PDU is neither taken nor acknowledged and comes again after a release; one event's receptions come in the air's order; PDUs held across a stop are read and released after it |
| `the_indication_holds_until_the_stop` | After each anomaly the facts say the port needs a stop; a round gives no event's end and no transmit end; the scheduler runs nothing; the stop's rounds give everything |
| `a_stop_ends_everything` | A stop from a start in progress; from a running radio with an event begun, one listed and a chain, after each anomaly; an alarm armed before the stop raises no fact after it |
| `a_second_run_is_as_the_first` | The record of each run and the state after each stop equal the first run's and the state before it: after every role's run, after each anomaly, after a failed start, eight runs in a row |
| `no_call_returns_an_error` | The port's calls, as the trait declares them, answer outcomes and rights |

Property 8 of the contract (the plan of the air) is the controller's. The
suite adds no executor: its polls use a no-op waker, and a port whose
answer is pending is a failure of the property that met it.
