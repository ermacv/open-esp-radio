# IEEE 802.11 installation machine over a port

`oer-protocols-ieee80211-testing-installation`: a testing package that
joins the [installation machine](../../core/installation/README.md) with a
real port. Its pump carries the machine's asks out on a `DataPort` through
the [conformance suite](../conformance/README.md)'s `DataPortHarness` and
feeds the port's answers back; its cases (`check`) are what every port
passes under the machine: a restart from a running port with traffic ends
reinstalled and the port runs again, a release ends released, a retirement
refused for a unit out reports `held` with its class and ends once the unit
comes home, and a stop that fails once is asked again.

It is its own package for dependency direction: the conformance suite is
the port contract's and stays below every core, and this package joins a
core with a port. The reference model and the S31 backend take it as a
dev-dependency only; no product package and no model takes it as a normal
dependency (`oer-tools-test-layers`).
