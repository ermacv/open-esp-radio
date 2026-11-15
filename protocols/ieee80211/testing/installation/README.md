# IEEE 802.11 installation machine over a port

`oer-protocols-ieee80211-testing-installation`: a testing package that
joins the [installation machine](../../core/installation/README.md) and
`drive` ([runtime](../../runtime/README.md)) with a real port. Its cases
(`check`) run `drive` on a `DataPort` through the [conformance
suite](../conformance/README.md)'s `DataPortHarness` and an
`InstallationHarness` beside it, which lends the installations a reinstall
takes and adopts the next installation's permissions. They are the
properties of the boundaries page, section 10, that need no role:

- from a run with a received unit held and a unit lent, a restart ends
  reinstalled and a release released, with nothing out of the port;
- after a fault the recovery takes back, a restart and a release end as
  they do from a run;
- a retirement refused for a unit held says what is out and ends once the
  unit comes home;
- a stop that fails once is asked again at the retry time and the halt is
  reached;
- a deadline ends the request, and the next request goes on from there.

A case's pump is a loop over the machine's rests; the parties beside the
port are a `Recorder`, as no data plane, controller or network runs here.

It is its own package for dependency direction: the conformance suite is
the port contract's and stays below every core, and this package joins a
core and the runtime with a port. The reference model and the S31 backend
take it as a dev-dependency only; no product package and no model takes it
as a normal dependency (`oer-tools-test-layers`).
