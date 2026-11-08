# oer-protocols-network-testing-model

One shared medium with several peers, for checking the network's egress on the
host, and the end-to-end simulation of the station and the network. No product
package depends on it.

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

## Station simulation

`tests/station/` composes the station and the network as the `hil-wifi-network`
image does, minus the hardware: the real station lifecycle, station and arbiter
cores and datapath actor over the IEEE 802.11 reference model's data port, the
station's neutral network end, and the network core, service and one channel
socket, all on one virtual clock. In place of the product it has the model's port
for the S31 backend, the software AES for the accelerator, a one-channel scan of the
test domain, no transmit Block Ack, and the facade's network stop and the
application's transmit memory without their statics.

The access point is a test peer on the model's air, not access point code. It
answers the station's probe with a beacon, its open authentication and its
association, and runs the authenticator's side of the four-way handshake with the
RSN authenticator, which checks the station's MICs. Once the port is open it relays
MSDUs to the wired side: a router that is the DHCP server, whose answer to each
REQUEST a case chooses, and a peer that answers ARP, pings and exchanges UDP. Its
protected frames reach the station as the model's hardware hands them up, decrypted
and verified.

A case is a script over the application's control, socket and address watch, the
access point and the clock. Tasks are polled only once woken; with nothing woken the
script looks again, then time moves to the next deadline, so a lost wake ends a case.
After the script the harness stops the station gracefully, runs every task to its
end and accounts for the network's blocks, its transmit buffers and the MSDUs left in
the station's transmit queue. A case of several sessions starts each after the first
from the owners the last stop returned: the actor reinstalls the model's port, a
station on the new installation's tokens takes the retired station's configuration,
the control opens a new epoch over the same command queue and state, and the network's
core takes the retired pool and brand, the same device port and transmit memory.

A case may drop the network service's `run` future mid-traffic: the harness's
network task, which outlives that future, then keeps the service, retires it and
serves the port's returns (`DevicePort::serve_returns`). That shows a host
composition with such an owner recovering; it does not cancel the S31
application's network task, which today is never cancelled.

What it shows is the protocol composition over the model's port: the real handshake,
DHCP, UDP, echo and the tasks' interaction under virtual time. The S31 hardware's
wakes, its HE path and transmit timing are outside it.
