# IEEE 802.11 channel arbiter

`oer-ieee80211-arbiter` is the arbiter of the channel one radio's station
and access point share, the third sans-IO machine of their loop.

The arbiter carries the lower-MAC `GateOpen<'id>` and `GateClosed<'id>`
directly. Both own the install's one `GatePlace`; only the backend's
retained `GateIssuer` turns a completed hardware barrier into closed
evidence. Arbiter actions transfer that place rather than abstract test
tokens or independently minted slots.

## Design decisions (owner, 2026-10-16; re-check when the station is built on the port at W3)

- **Planned retunes.** `plan_absence` and `plan_move` take typed permits. A
  `PlannedRetune` token lives inside the lease and returns through
  `withdraw_plan`. A refused or late plan does not cancel the lease. The
  preparation window is at least `CLAIM_LEAD` plus one service step. Scan
  visits are planned retunes too.

An arbiter-backed installation declares `ArbiterPool` in its complete pool
set. It yields the open gate, backend issuer and one `ArbiterAuthority` at
the same component path. `Arbiter::new` consumes that authority, so handing
a gate out for a cycle cannot construct a second arbiter with new lease
places. Primary/absence places, lease IDs, leases, replies, notices and
pending retunes carry the invariant installation lifetime and gate path.
Sibling components and separate installations cannot exchange those values.
Station tokens retain this path independently of the connection epoch path.
Lease identities never wrap: exhaustion returns `IdentityExhausted` and the
original admission place before any announcement. An exchange that cannot
issue a new identity returns `Err(old_lease)`; that lease still releases its
original admission place. An unchanged or non-primary lease is also an
explicit refusal, not a successful exchange.

Only the arbiter changes the port's channel: it closes the transmit gate,
retunes the port and opens the gate again. The station asks it for a channel
(`Arbiter::request`) and gets its answer in the same step: a named `Lease`
of the channel from `from` until `until`, or a command to leave. Notices of
the lease start (`LeaseStarted`) names it. End and cancellation return the
unique `LeasePlace` without an ID: possession proves the sole lease ended.
Absence end and cancellation likewise return the unique `AbsencePlace`. Beside a
running access point, the station leaves the access point's channel only in
absences after its TBTTs; a station that tells its access point it leaves
says it is ready, or the lease is cancelled at its start. The lease's
`until` is the moment the port is back on the access point's channel, so
its beacons hold; the station listens until the return's retune, and a
lease whose gate closes too late for two retunes and the shortest useful
listening is cancelled. The access point's air reservation is a policy
option, off by default: the gate's closing takes one last frame, a
CTS-to-self the driver sends with the Duration left.

Admission places, answers, notices and actions are `must_use`, as is each
pending retune. Polling transfers work or a deadline to the caller; silently
discarding that result is diagnosed. Explicitly dropping an affine value
remains possible and does not complete its protocol obligation.

A join on another channel beside a running access point starts with the
access point's announced move there, by the `ApFollowPolicy`; the move is
never withdrawn, and a second such join within the policy's
`min_follow_interval` is refused with the moment to retry.
If the holdoff's end does not fit monotonic time, `ScheduleExhausted` returns
the admission place without announcing a move, consuming an identity or
changing the last-follow time. Overflow cannot bypass the holdoff.
The switch instant and one further beacon interval must both fit before
the arbiter commits an announcement or its follow backoff. If `SwitchDue`
does not arrive by that deadline, `SwitchTimeout` returns the station's
place once. A later AP switch cannot start that cancelled lease. Stopping
the AP lets the join proceed alone at its original earliest start.

`coordinator` keeps the access point's follow, search and loss policies
(`ChannelCoordinator`, `ApFollowPolicy`, `ApSearchPolicy`,
`UpstreamLossPolicy`), and `channel` the channels an access point may not
serve without radar detection. Coordinator facts, schedules and actions
carry `SupportedChannel<K>` from the backend's capability check. Applying
a start or move therefore needs no second channel conversion that could
discard the command after its state transition. The follow policy's
announcement count is nonzero at construction; it reaches the beacon
announcement without silently increasing a zero count to one.

Composition passes the backend's `PUBLICATION_BOUND` to
`ArbiterPolicy::checked::<ReservationMode>` before `Arbiter::new` consumes
the gate and authority. Construction rejects inverted follow intervals,
an absence too short for two publication/retune bounds plus minimum
listening, and arithmetic overflow. Reserving mode also rejects absences
longer than the Duration field; choosing `NoReservation` is explicit.
Absences reserve one current publication plus retune time for each move;
`listen_until` and the return deadline leave this budget before the absence
ends. Once the outgoing gate has closed, the late-lease check counts only
the remaining return publication. Retry-plan length never enters this bound.

Each retune carries a private `PendingRetune`, which retains its destination
and cycle purpose. The service owns it across disable, tuning and enable;
`retuned` or `retune_refused` consumes that exact action token. A channel
value alone cannot acknowledge a retune or create an unsolicited response.
While a gate cycle runs, the next input is its gate or retune terminal;
`poll_deadline` returns no timer. Planning a shared move consumes its deadline
while the pair remains the sole owner of the access-point schedule. The cycle
owns its move and next move, without a second schedule copy. An absence
ends no later than its primary lease; one starting after that deadline returns
`PrimaryEnds` with the deadline and its place. A waiting absence
is cancelled when that move becomes due. The active move retains a subsequent
coordinator retune until its gate opens; completing the first move then arms
the next one. A TBTT fact on the old channel updates beacon timing without
changing the active move's home destination. Lease and upstream-loss deadlines
resume after the cycle ends, and an elapsed deadline causes a finite transition.
Late admission is checked before the outgoing retune. Once that move is
admitted, its completion cannot charge the outbound budget again; the lease
ends on its original return deadline. Refused and late moves capture the
return channel in `CyclePurpose::Return { to }`. One `listen_end` calculation supplies the
lease, timer and return transition. A leave
command deferred by an absence belongs to its `AbsenceTerminal`, never a
primary lease. Cancellation during Closing retains its original cause and
skips the cancelled outgoing retune. Every terminal, including
refusal and late cancellation, returns its place and releases the command.
Releasing a lease during Closing is also a pre-start cancellation; it
reopens the gate without an outbound retune or `LeaseStarted` notice.
A refused lease move consumes the arbiter. `RetuneRefused::Return` gives
back the working arbiter and the home `PendingRetune`, already in `Retuning`.
A refused return or shared move yields `RetuneRefused::Failed`. Without an
enabled phase, consumers poll `ArbiterNotice` through `poll_notice`; that
exhaustive type contains only station/AP notices and cannot take a gate or
retune token. `finish(self, ArbiterEnd)` consumes planning and coordinator
authority. `PortFailed` returns `FailedArbiter`, which only drains its outbox
and retains late gate/retune returns. `Stopped` cancels leases with `Stopped`
and returns a distinct `StoppedArbiter` bank. Idle station places and cancelled
lease places are retained in `StationPlaceReturns`; they have no restart
authority. A successful restart returns the complete `StationPlaces` owner
alongside the arbiter. Its fields are private, so partial return collections
cannot be passed into a station restart. `StoppedArbiter::restart(channel, returns)`
preserves the policy, gate and monotonic lease sequence without minting another place.
An unreturned gate terminal or admission place yields `IncompleteArbiter`
with both resource collections; a closed gate is reopened through the new coordinator's first cycle. It has no request, ready, retuned or coordinator input methods;
draining requires no clock and cannot issue another AP Start. Failure cancels
each still-held admission place once, withdraws a queued AP start/announcement
and preserves already owed terminals. Failure recovery needs a new installation and its unique `ArbiterAuthority`;
a normal Stop reuses its returned bank.

The station chooses search channels and requests their leases. The arbiter
places those absences around the access point's beacons and spaces dense
and sparse searches; the coordinator keeps follow and loss state without
an independent poll loop or channel cursor. `UpstreamFact::JoinFailed`
resumes the search from the original loss instant, including its loss
policy deadline. Joining does not grant a new search period. The coordinator
retains a failed rescan's AP restart until the return cycle has completed
on the fallback channel and reopened its gate. A refused return never
publishes that restart, including when the scan ended on another channel.
The coordinator
cannot be copied into another owner. The crate uses the canonical strict
lint block.

Beacon schedules carry `BeaconInterval`, which rejects zero at construction.
The arbiter and coordinator use the same TBTT grid: absences start at TBTT
plus the guard, while a switch uses the TBTT itself. The next slot uses a
checked difference, ceiling division, multiplication and addition; its cost
does not depend on missed beacons. If no future slot fits the clock range,
`ScheduleExhausted` returns the admission place without creating a lease.

## Known defects

Found in the predecessor's review of 2026-10-16 and still present.

- **An absence returns to the home channel, not to the primary lease's.**
  `lib.rs::Arbiter::lease_cycle` uses `Granted.returns: bool` and
  `self.home`; after a CSA follow the station comes back to the old channel.
- **Spin on an expired deadline.** `lib.rs::request_absence` refuses only
  `from >= until`, `plan` skips the primary during an absence, but
  `poll_deadline` still returns its past `listen_end`: an absence whose
  `from` falls between `listen_end` and `until` makes the service wake
  continuously.
- **Flag and placeholder state:** `following`, `Granted.returns`,
  `LeaseStep::Waiting { ready }`, `Outbox.stop` bools; `Outbox` `Option`
  cells overwritten by assignment; `poll_action` uses
  `mem::replace(&mut self.gate, Gate::Closing)` with restore arms;
  `Port::Retuning` has no target.
- **Low:** `lib.rs` `finish`/`terminate` always queues the AP stop;
  `coordinator.rs::beacons_before` clamps to `u8::MAX` instead of deferring
  the announcement; `Granted::listen_end` falls back with
  `checked_sub(..).unwrap_or(until)`; the follow interval may double after
  an AP `Stopped` (not traced); no test for a double announcement on `Found`
  on a foreign channel.
- **This README's search description** (the station chooses search
  channels and requests their leases; `JoinFailed` resumes) describes
  behaviour the station does not emit yet (see the station's known defects).
