# oer-protocols-network-testing-model

One shared medium with several peers, for checking the network's egress on the
host. No product package depends on it.

`Medium` is a port and the radio's view of its peers at once: a frame holds the air
for a time set by its peer's rate and retries, one frame at a time, and completes
with what it cost. What the radio estimates is apart from what the air costs. A peer
can sleep or change association, the port can refuse, and a cost can be unknown.
It knows no scheduler, and it answers ARP so the real stack resolves its peers.

The scenarios in `tests/` run the real sockets, service, core and policy, and judge
by what the medium counted, which the policy never sees. `Direct` runs beside the
airtime policy as a control: a scenario that could not tell them apart would prove
nothing.

It models contention for one radio's air: rates and retries are fixed per peer, and
it has no loss, aggregation, collisions or other stations' traffic. Its shares are checks
of the scheduler's arithmetic and ownership, not claims about real air.
