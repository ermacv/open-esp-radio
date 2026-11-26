# IEEE 802.11 backend conformance

`oer-protocols-ieee80211-testing-conformance`: the shared conformance suite of
the [IEEE 802.11 port contract](../../contracts/port/README.md). It is a
testing package: a backend takes it as a test dependency, the reference
model (`oer-protocols-ieee80211-testing-model`) as a dev-dependency, and no
product package depends on it (`oer-tools-test-layers`). It takes the port,
the wire formats, the security contract (the key a run installs) and
portable support alone.

A backend's test implements `Harness<L>` for each layout it serves (its
port, a channel, network memory and an interface's link, frames landed on
the receive ring, the anomalies it can raise, a MAC held busy, the next
command made to expire, the chains a queue carries, the air held and released, what the hardware was asked to
program and its state, whether its own octets of its transmit slots are
whole) and calls each property with an installation's
rights (`Rights`). The suite keeps the rights in its own places and checks
the port's answers against them; it reads no bookkeeping of the port's.

| Property | What it checks |
| --- | --- |
| `every_right_is_in_exactly_one_place` | A run, its data path (the gate opened, a pairwise key installed and removed, the gate closed) and its stop hand every right back; a retirement takes them whole and the release ends the port |
| `a_second_run_is_as_the_first` | What the hardware programs in a run after a stop equals the first run's, and its state after each stop equals its state before the first start; after each anomaly, and after a stop whose last resort ran; the gate is closed at each start and the data's rights are home after each stop |
| `a_stop_ends_everything_on_the_air` | A single on the air when the stop comes ends withdrawn through the sink; its place and queue come back |
| `the_indication_holds_until_the_stop` | An anomaly raises "the port needs a stop" in the facts, with no payload, until the stop |
| `a_unit_is_in_one_place` | A unit taken in a round stays in its place, named by its handle; one not taken goes back at the close |
| `a_publication_sends_the_prefix_its_rate_admits` | An aggregate at a rate that admits less than the arena holds sends the longest prefix as an A-MPDU; the rest stay with no attempt spent |
| `a_single_publication_sends_the_first_frame` | A single publication sends the first frame as one MPDU; the rest stay |
| `the_ports_octets_stay_the_ports` | Frames of the longest body the port's slots hold are written and published; the port finds its octets in front of the headroom (`TX_FRONT`) and behind the body and its MIC's room (`TX_BEHIND`) whole after the writes and at the publication, as at every publication of the other properties |
| `an_aggregate_of_one_frame_ends_single` | An aggregate whose rate admits one frame sends it alone by its plan, and its outcome says one MPDU went |
| `an_unclassifiable_frame_goes_back_at_its_staging` | A unit that is no data, management or control frame is counted and returned to the ring; the round hands the others |
| `a_round_services_the_ring_once` | A round services the ring once and hands what that service staged |
| `no_call_returns_an_error` | The data path's calls, as the trait declares them, answer outcomes |
| `chains_on_two_queues_end_each_with_its_queue` | A chain on best effort and a chain of one on voice are on the air together, each on the queue it was published on; each ends and its settlement returns its own queue's right, its frames' memory home |
| `a_stop_returns_every_chain_and_queue` | With chains on two queues on the air, a stop ends both; their settlements return each queue's right and every frame's memory, and a retirement takes every right |
| `a_clients_key_goes_alone` | On an access point of two clients: each client's key is installed in its own place; the first's removal leaves the hardware with the second client's key alone |
| `the_receive_rule_is_a_right` | The station's rule is set with its address, changed and ended, each end giving the right back; a setting that expires gives it back off and leaves the hardware as before it; a change that expires gives it back off with the rule cleared |

The suite adds no executor: its polls use a no-op waker, and a port whose
answer is pending is a failure of the property that met it.
