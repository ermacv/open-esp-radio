# IEEE 802.11 backend conformance

`oer-ieee80211-testing-conformance`: the shared conformance suite of the
[IEEE 802.11 lower-MAC port](../../contracts/port/README.md). It is a testing
package: a backend takes it as a test dependency, the reference model
(`oer-ieee80211-testing-model`, itself a testing package) as a normal one, and
no product package depends on it (`oer-test-layers`). It takes the port, the
wire formats and portable support alone.

Backend tests implement its ownership, publication-barrier, configuration, GTK and outcome
harness traits; they need no dependency on `oer-ieee80211-runtime`. The model
in `oer-ieee80211-testing-model` runs the same runners with virtual time. Traces contain observations
from actual submitted/returned owners, publication observers and event streams;
expected identities cannot substitute for observations.

Each case exposes `execution()`: `Hardware` or `ModelOnly(reason)`. Runners take
`Environment::Model` to exercise everything, or `Environment::Hardware` to
exclude injected conditions explicitly. Their `Report` counts verified cases
and model-only exclusions separately. The physical fixtures still supply real
ACK/BlockAck evidence and actual backend progress; classification alone does
not prove a hardware implementation.

| Suite | Hardware cases | Model-only cases |
| --- | --- | --- |
| Ownership (`check`) | Complete, close gate, disable, admission refusal | Poisoned backend |
| Publication (`check_barriers`) | Gate with ACK, Sent retry, key removal | Lost hardware completion |
| Configuration (`interface_commands::check`) | Both interface orders, running/queued/posted disable | None |
| Outcomes (`outcomes::check`) | Single ACK, aggregate BlockAck, single/aggregate NoResponse | Single/aggregate Collision, AirDenied, HardwareTimeout |
| GTK (`group_keys::check`) | Disable before/after each of four mutations, published-key removal | Four independently delayed-ingress RX cuts, four stalled-command expiries |
| EDCA (`edca::check`) | Two VIFs with different tables, every AC | None |
| MSDU age (`msdu_age::check`) | None | Before publication, between retries, re-formation with a core BAR, delivery before head expiry, hardware-owned completion |

Hardware ACK uses a reachable peer and NoResponse an absent address. Collision,
AirDenied and HardwareTimeout require model injection. The GTK receive-cut
cases independently inject old-key facts at hardware, DMA, interrupt and
software stages; ordinary RF traffic does not establish those exact cuts.
Command expiry and fail-stop also require explicit faults. Hardware cases
that intersect a held publication or a queued command require cooperative
backend progression and observation at the specified boundaries.

The suite adds no allocator or executor dependency. Harness associated types
select trace storage through `AsRef<[T]>`: host fixtures use `Vec`, and an image
can use arrays or bounded collections. `BarrierHarness` also selects frame and
retry storage. GTK checks take an independent `oer_time::Timer` to bound the
whole fixture, including a backend that never delivers its terminal; model
fixtures pass a virtual timer. A standalone consumer in `tests/conformance.rs`
exercises runner selection and negative verifiers with arrays and `heapless`.
