# Wi-Fi runtime

`oer-protocols-ieee80211-runtime`: the pumps that carry the sans-IO cores'
steps out on the lower-MAC port. A pump decides nothing and holds no
machine: it is a loop over its core's steps.

`installation::drive` carries one step of the [installation
machine](../core/installation/README.md) out, for every role and every
test. It answers each ask on the port, or on the data plane, at once,
passes each message and end on to its party (`Parties`), and returns the
step's rest. It is a loop of one `match` with no field, and the only caller
of the port's transitions in the repository, by type: six of the eight
take a proof and two the stop's right, which only the machine holds.

Three answers of the port carry an owner that is not the machine's, and
`drive` splits them: a reinstall's `Installed` goes to the controller in
the one call that delivers the restart's end (`Parties::reinstalled`), a release's device stays with the binding that holds the
port, and an item a cleanup hands back (`Unkeeping`) goes to the data
plane. The machine gets what is left.

Its test is `oer-protocols-ieee80211-testing-installation`, which runs
`drive` over the reference model and the S31 port.
