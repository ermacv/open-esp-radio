# oer-protocols-network-testing-model

One shared medium with several peers, for checking the network's transmit path on
the host, and the end-to-end simulation of the station and the network. No product
package depends on it.

`Medium` is a port and the radio's view of its peers at once: a frame holds the air
for a time set by its peer's rate and retries, one frame at a time, and completes
with what it cost. What the radio estimates is apart from what the air costs. A peer
can sleep or change association, the port can refuse, and a cost can be unknown.
It knows no scheduler, and it answers ARP so the real stack resolves its peers.
As the station's buffers do, a frame's ticket is its buffer's place below the
port's depth, free again only once its notice is taken. `Medium::batch` tells the
ends of that many landed frames at once, or fewer once the air is idle: a Block
Ack's settlements, without modelling the aggregate's air.

The scenarios in `tests/` run the real sockets, service and core, and judge by
what the medium counted, which the service never sees.

It models one radio's air: rates and retries are fixed per peer, and it has no
loss, aggregated air, collisions or other stations' traffic. Its scenarios check
ownership and the service's progress, not claims about real air.

`tests/mixed_service.rs` instead composes the real service, local producer,
`DevicePort`, station `LocalTx` and its outgoing and returned slot queues, with
lent receive frames. It fills the transmit pool, leaves prepared work waiting
for room, then returns one buffer together with receive readiness, just after
the readiness snapshot, or after a receive turn. The executor polls only after
a wake. The cases check that one returned buffer suffices to publish, including
with more received frames than one intake batch, and bound the receive turn
that may precede it. The event tape also records producer refill followed by
`NoRoom`: a turn with none of the four events counted by the load image's
"empty" statistic. This composition models no RF time, hart cost or throughput.
With the longer RX queue the same refill and refusal accompany a received
frame, so that turn has a counted event.

`mixed_service/credit.rs` uses the image's 12 RX credits and 8-frame intake
budget. Its two tasks follow the S31 executor's ready snapshot and network
before application order. It compares per-frame and batched publication with
identical arrivals and consumer opportunities, and records socket-queued and
application-held owners, physical returns, published loan returns and credit
restoration separately. A draining consumer, a four-frame hold, a full-credit
hold and a TX return exercise progress without changing credit or batch sizes.
Those original scenarios remain regressions of the explicit servicing
contract. Additional cases use synchronous application prefixes and batched
credit publication, including individually retained owners and the reverse
task order. A consumer which cancels after publication or stays busy after a
prefix cannot block RX or TX; genuine held/queued credit still bounds its
delivery. The publication offer defers one intake pass only and is consumed
by the service itself. Runtime regressions also keep an unfinished receive
future unpolled while another copying or lending socket receives, and keep
two such futures while ARP and stop progress. The model continues to separate
physical return, published credit and the service's collection, and checks
all owners at retirement.

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

## Work per frame

`tests/scenarios/work.rs` composes the transmit path as the load image does:
the real service and core, the image's network geometry and a
local producer of load datagrams, a header and filler, unpaced, to one peer.
The medium's air is so fast that the service bounds the load, as a hart it
saturates does, and it tells a Block Ack's 32 settlements at once. The
service's diagnostics, always on here, and the producer count the work: turns,
places walked, fills and offers.
A case bounds that work per frame.

The counts are the work's shape. Its weight is the case's instruction count on
the host, which a run pinned to one core makes exact to a few hundred
instructions a run:

    cargo test --release -p oer-protocols-network-testing-model --test scenarios --no-run
    taskset -c 0 perf stat -e instructions:u <the scenarios binary> \
        work::an_unpaced_load_takes_each_frame_once_and_walks_one_place_for_it --exact

The medium, the harness and the virtual clock are part of that count and do
not change with the network's code, so the difference between two revisions,
divided by the frames, is the work per frame one removed. `perf record` with
the same event attributes it by function. A host instruction is not a hart's:
the host vectorizes byte loops such as the checksum, which a hart without
vector instructions runs one word at a time, and memory stalls, which a hart
running from PSRAM pays for every copy and miss, are outside the count. The
virtual clock wakes every wait at each advance, and the medium advances at
each frame's landing, so the service waits once a frame here, which a
saturated hart never does.
