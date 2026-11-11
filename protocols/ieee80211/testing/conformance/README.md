# IEEE 802.11 backend conformance

`oer-protocols-ieee80211-testing-conformance`: the shared conformance suite of the
[IEEE 802.11 lower-MAC port](../../contracts/port/README.md). It is a testing
package: a backend takes it as a test dependency, the reference model
(`oer-protocols-ieee80211-testing-model`, itself a testing package) as a normal one, and
no product package depends on it (`oer-tools-test-layers`). It takes the port, the
wire formats and portable support alone.

Backend tests implement its harness traits; they need no dependency on
`oer-protocols-ieee80211-runtime`. Traces contain observations from actual
submitted and returned owners; expected identities cannot substitute for
observations.

| Suite | What it checks |
| --- | --- |
| Data port (`data_port::check`) | No call wakes its caller, refusals return their owners, a receive pull stays within the caller's room, a configuration waits at the published arena as `Held`, the deadline follows admission and completion |
| TX arena (`arena::check`) | Intake until full, release order, withdrawal, publish refusals; partial BlockAck retention, protection-failure republication and a fault in the model only |
| GTK receive cut (`group_keys::verify`) | The old key's delayed DMA, interrupt and software ingress before the mutation's terminal, the returned installation's identity, slot and counter, the command bound |
| EDCA (`edca::parameters`) | Distinct complete tables a fixture installs per interface |
| Access point (`access_point::check`) | One port for both roles; each role's configuration barrier holds only its own role; the beacon permission's one beacon, its expiry at its deadline, and no beacon on a stopped TSF; an earlier installation's configure permission, beacon permission, key slots, removals and frame keys refused with their owners; key installs and removals; the access point's stop alone, with held commands, and a shared stop |

Each arena and receive-cut case exposes `execution()`: `Hardware` or
`ModelOnly(reason)`. Runners take `Environment::Model` to exercise everything,
or `Environment::Hardware` to exclude injected conditions explicitly; their
`Report` counts verified cases and model-only exclusions separately. The GTK
receive cuts inject old-key facts at hardware, DMA, interrupt and software
stages; ordinary RF traffic does not establish those exact cuts.

The suite adds no allocator or executor dependency. Harness associated types
select trace storage through `AsRef<[T]>`: host fixtures use `Vec`, and an image
can use arrays or bounded collections.
