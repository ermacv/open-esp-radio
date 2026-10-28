# Portable Wi-Fi station policy

This crate contains the station, independent of a chip, executor and network
stack. It is sans-IO: `machine` is one state machine for the whole lifecycle
(scan, SAE or Open System join, RSN four-way handshake, connected, background scan, group
rekey, leave) that takes the port's events, the channel arbiter's answers and
notices, commands and time as values and leaves the port's and the arbiter's
actions as values. A thin service loop over the lower-MAC port feeds it.

Produced actions, transmissions and pending response tokens are `must_use`:
the consumer performs or forwards them before waiting again. Rust can still
explicitly drop an affine value; the annotation diagnoses accidentally
discarded results and does not replace terminal resource handling.

`Station::command` consumes a `StaRequest` together with its unique command
credit. Construction returns separate Connect, background-scan and Disconnect
credits, bound to the installation and interface by their types. A refusal
returns the original request synchronously; admission retains its credit
through the operation and terminal delivery. `CommandCompleted` returns the
same request and credit, so the application can issue that kind again. A
pending request or undelivered terminal cannot coexist with another credit
for its kind. Disconnect keeps its independent terminal cell and cannot
overwrite a pending scan result. Lifecycle Connected/Disconnected notices
remain distinct from request completion. A scan result retains partial
observations and names the exact absence refusal, cancelled-absence limit or
link-ending reason. Correlation is caller data and confers no authority.

After the port's terminal failure, `Station::finish` consumes the protocol
machine into `RetiredStation`, with separate epoch, key, frame and
command resource fields instead of a runnable station. This path retains its
owners and emits only the
original request terminals and one lifecycle notice; it cannot issue Configure,
key removal or TX effects to the failed port. Idle channel places return
separately in `StationPlaceReturns`, without advancing scan/join transitions. The stopped arbiter consumes this collection
and the cancelled lease returns, then hands out the complete `StationPlaces`
by value. The infallible station restart requires that complete owner.
`StaData::finish` returns queued frames to its own terminal bank. A complete station bank can
consume itself through `restart(self, StationPlaces)`, preserving its epoch
source and all minted resources. The resulting station reclaims old keys and receive configuration
before accepting commands; the data bank preserves admission permits and its
ordering frontier. Deferred EAPOL owners return separately for the old association.
Only the service's ordinary Stop continuation supplies the reusable disabled
port owner needed to run these banks again. Healthy disable returns every frame
and key owner before these banks or the installation are destroyed; poisoned
work still owned by the backend remains in that installation's retirement custody.
The `test-support` feature exposes the retired station's resource counts for
composition tests; those diagnostic methods are absent from product builds.

Operation deadlines use checked monotonic arithmetic. Authentication,
Association and handshake overflow end Connect with `DeadlineOverflow`.
A background dwell overflow ends only its original scan request with
`BackgroundScanEnd::DeadlineOverflow`; a delayed widened-receive terminal
first restores member receive rules, then returns the scan terminal. The
working channel, gate and primary lease remain available to the connection.
If cleanup fails or a deadline required by the connection itself cannot be
represented (including SA Query), the link ends with the concrete failure
reason. An overflowing deadline is never replaced by `now` or a wrapped time.

`machine::Station` accepts Open or personal credentials carrying both a
zeroizing SAE password and a prederived PSK PMK. Selection prefers qualified
SAE, including a transition BSS offering both SAE and PSK. Join performs the
selected hunting-and-pecking or hash-to-element exchange; two calls to the
composition's fresh-randomness provider create its commit. Successful peer
confirmation moves the negotiated PMK through association into the four-way
handshake. The provisioned PMK is used only for a PSK offer.

SAE observations consume the exchange and return it only while it continues.
Commit and confirm have checked absolute deadlines, including time waiting
for a TX credit. An anti-clogging token repeats the same commit without
extending its deadline. RX may advance before the outgoing frame's TX
terminal; that terminal returns resources and cannot restart an earlier
exchange. Timeout, peer refusal and invalid confirmation retain their concrete
failure causes. Pending transmissions retain their bytes until credit returns.

The station retains up to ten SAE PMKSAs by BSSID and SSID. A repeat connection
uses Open System authentication, names the cached PMKID in its association
request and proves the cached PMK in the handshake. A PSK association uses
neither that PMK nor that PMKID. Host wire peers validate SAE-only, transition,
PSK-only and cached connections, including the transmitted M2 MIC. They are
not ESP32-S31 hardware evidence.

The local transmitter is `OwnAddress`; a scan record validates its individual
BSS address as `Bssid` before it can enter Join. Authentication, Association,
SAE, the handshake, the connected link and station frame encoders keep those
roles. Wire writers unwrap the octets at the encoding boundary, so beginning
an attempt cannot uncover a previously unchecked address.
Association also retains every advertised legacy rate from Supported and
Extended Supported Rates in `StaAssociatedPeer::rate_peer`, alongside the
separate basic-rate set for protection. A response cannot create a peer from
an advertisement containing no usable legacy rate.
Authentication TX contexts also retain the attempt ordinal inside their link
epoch. A late terminal or preparation refusal returns its credit and buffer
but cannot end a later RX-driven retry or arm that retry's response timer.
RX may still complete the current exchange before its TX terminal arrives.

Open links and pre-key authentication use MAC's disconnect parser. A PMF
handshake retains a software management CCMP receiver when it installs its
pairwise key. The IGTK becomes active at the group installation terminal;
connected state owns both receivers and their independent PN/IPN frontiers.
Only authenticated management from the selected BSSID can disconnect that
link. Hardware integrity metadata alone grants no authority. A repeated M3
retains the pairwise frontier, and reinstalling the same IGTK keeps its IPN.
The owner and its software secrets end with the association.

An unprotected class-2/3 disconnect starts the existing SA Query procedure,
using fresh randomness from `StaConfig::callbacks`. The concrete `StaCallbacks`
provider also supplies the composition's HE padding policy and retains its
state through station retirement and restart. Requests and responses go
under the installed pairwise key; only a matching protected response ends
the query. Retry and absolute timeout wake independently of receive traffic.
One pending response is retained while control TX is busy; additional peer
requests rely on retransmission. A protected Deauthentication keeps its key
until its TX terminal or preparation refusal, before key removal begins.
These paths have host wire-peer tests; they are not ESP32-S31 hardware
evidence.

Module map:

- `machine`: the station, its tokens (`StaTokens`) and its host slice tests
  beside the real arbiter;
- `block_ack`: recipient ADDBA parameters, typed refusals, inactivity and
  reorder lifetime. The service supplies affine bank and retained-frame
  owners. The machine validates association addresses and PMF before handing
  over Block Ack management, and uses its control TX credits for replies;
- `data`: the connected station's data machine beside it: MSDUs into QoS or
  non-QoS Data MPDUs in its own tickets and buffers, under the pairwise key
  the station's `LinkView` lends, and the BSS's data MPDUs into MSDUs; no
  A-MPDU, Block Ack, reorder, replay check or power save yet;
- `join`: Open System and SAE authentication and Association state machines
  and their retry policy;
- `association`: association capability selection from the scan record;
- `power_save`: what the access point knows of the station's power state and
  the TIM's decision;
- `station`: the application's vocabulary (lifecycle stages, fault reasons,
  reconnect policy);
- `modem_sleep`: the connected station's power manager, after the vendor's
  `pm.o` and `pm_coex.o`, with the vendor's timing constants;
- `sa_query`: the SA Query procedure of an association under management
  frame protection;
- `link_monitor`: beacon-loss decisions;
- `pmksa`: the private SAE PMKSA cache owned by the station across connections;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation. FTM local
  exchange timing carries a `NonZeroU8` dialog token from validation through
  replay tracking; terminal token zero cannot enter that state. The TWT wake
  plan is computed in `TsfInstant`s of one TSF generation the caller tracks;
  its intervals and guards are `TsfDuration`s. A guard before the epoch is
  `BeforeTsfEpoch`, and an endpoint beyond the epoch is `BeyondTsfRange`;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.

`StaAssociationRuntime` owns one of the initial request epoch, comeback wait
or returned request epoch. Both polling and management RX consume this owner;
only a continuing result returns it. Success, rejection and timeout consume
the association, so no terminal flag or repeatable terminal operation remains.
A comeback retains the received-frame count and gets a fresh request schedule
and response deadline after its wait. A second temporary refusal ends it.

FTM starts by consuming `FtmRequester` into an owned `FtmSession`.
Its peer is a checked `IndividualAddress`. `FtmOperationId` has private
fields and is obtained from that session; its getters expose diagnostics.
`service` transfers that session into the transmission handle until
`complete` or `reject_hardware_admission` consumes it; a completion takes
no separate requester argument. The copyable generation numbers are
diagnostics within one requester, not completion authority. Only an owned
`FtmTerminal` returns the requester for another operation. A competing
request retains its value and names the current `SessionTerminal` as its
wait source; waiting sessions expose their exact `next_deadline`.

Cancelling a session with a completed RX result returns that result.
Cancellation and failure otherwise return collected samples plus the
unmatched local timestamp pair. Scheduling overflow returns the unchanged
requester, session or transmission before advancing its state. Forgetting
an affine handle forfeits that operation; it cannot unlock or complete a
different one. These ownership contracts establish no FTM
timestamp-capture or calibrated-distance capability.
Samples live in one bounded `heapless::Vec`, whose length is the count.
The terminal takes this vector by value, preserving order through completion,
failure and cancellation without sparse entries or a separate counter.

Frame parsing/building belongs in `protocols/ieee80211/wire`; ESP32-S31 ordering
and hardware ownership belong in the chip driver; clocks, tasks, DMA wakeups
and network leases belong to the radio runtime; final resource claims and
board composition belong to integration.

The station's SSID and SAE bodies own their bounded lengths in heapless
vectors. The SSID bound comes from MAC's `MAX_SSID_LEN`. Scan channels form
a bounded, contiguous vector of checked `SupportedChannel` values, so a
hole cannot terminate a scan before its remaining channels. If a received BSS
frame omits a channel element, the scan uses that frame's checked receive-channel
configuration from `RxMeta`, including delayed delivery after a lease changed.
The current lease or absence is not a substitute for that captured configuration.
WMM priorities and TWT flow IDs select exact arrays without numeric
indexing; an unpublished data return retains its checked frame description.
Data writes an open MAC header directly into its proven span, so it fits an
exact-size buffer without a temporary CCMP gap or a second payload move.
The PM owner retains a NonZero beacon period through its divisions. Listen
intervals and TBTT periods keep their full `u64` value. Monotonic arithmetic
is checked: a short slice emits `Due`, and a future slice or preemption uses
an absolute `ArmAt` deadline, so delayed execution cannot extend it. Fallible
clock inputs and interval products return `PmScheduleError` without changing
state or publishing effects. It validates a transition on a value-only copy and commits that
copy; the transition has a finite effect bound and no allocation. Relative
timers are validated against one captured clock before a consumer applies the
batch; a backend must also reject intervals its hardware cannot represent.
`ModemSleep::start` returns a fallible batch as well. Only the activity
presence counter retains intentional modular rollover.
Each modem-sleep input returns a fresh `PmActions` batch. Before changing
state, its helpers receive fixed arrays of affine effect places: writing
consumes a place, so neither overflow nor overwriting another effect is
possible. TBTT reserves the largest batch: 1 normalization + 1 RF wake +
13 coexistence + 1 interval + 8 parameter effects, totaling 24. Other inputs
reserve fewer places. Consumers execute each returned batch in order;
separate inputs cannot append into the same batch. The TX offer returns its
hold decision alongside the effects, including when the frame must wait.
The station owns `Counter`, the saturating diagnostic value its data path
and service loop report. It carries no lower-MAC contract, resource capacity
or protocol-state decision.

The crate uses the canonical strict lint block. Compile-time checks use
private functions scoped inside const expressions, with the explicit
`expect(clippy::panic, reason = "evaluated at build")`. Those functions cannot
be called from the runtime path; their checked values remain in closed types.


## Transmit ownership

`StaConfig::listen_interval` uses the checked nonzero `StationListenInterval`
in beacon intervals. Association advertises that exact value; the composition
chooses it together with its power-save policy, without a hidden station default.
`StaConfig::access_point_margin_db`: owner policy 6 dB, supplied by the
composition.

The interface configuration carries `OwnAddress`; an Open Authentication
request takes it separately from the peer's `Bssid`. Both wrap the MAC
crate's checked `IndividualAddress`, so a request cannot swap the local
transmitter and BSS identities. The test access point admits a request only
after checking its subtype and all three header addresses.

`StaSecurity::Personal` carries both the SAE password and PSK PMK; AKM
selection follows the BSS qualification.

`Station::new_open` and `Station::new_personal` return the machine and its
three command credits. They require one to eight tickets and small buffers
at compile time. Resources must match the backend token
type's `StaTokens::GROUP_SLOTS`. Open mode accepts zero group slots; the
personal constructor requires at least one: it takes a `GroupCapacity`
proof, which a `const` item evaluates (a zero capacity fails the build), and the
SAE password and PSK PMK separately from the common configuration. Authentication and Association reserve an owned
ticket/buffer credit before beginning a request or consuming its sequence
number. Authentication's retry owner becomes an active exchange only after
that reservation. A beacon, Probe Response or Authentication Response while
waiting for credit cannot finish an exchange that has not begun. The active
owner moves through TX and response waits; only its retry outcome permits
another attempt, with no separate active/terminal flags.
EAPOL (including the QoS rekey response), Null, deauthentication
and Probe reserve credit before consuming a sequence. A missing credit
waits for a terminal that returns the required ticket or buffer from the
station pool; it is not tied to one named operation. Association starts its
deadline when entered, before any credit or first transmission; its overdue
retry time
does not cause repeated polling. Every four-way handshake step retains the
same response deadline while waiting for TX, a key slot or installation.
Expiry leaves once and reclaims late returns through the normal teardown.
After reserving credit, encoding can fail only with `EncodeError`. Null
transmission waits only when credit is occupied; an encoding failure leaves
once and returns its ticket and buffer, and a missing connection is reported
as a changed link.

GTK wire IDs include zero. IEEE 802.11-2020, 12.7.2 (EAPOL-Key frames),
defines the GTK KDE's two-bit KeyID. Frame addressing distinguishes PTK
and GTK. The pinned
[ESP-IDF parser](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/wpa_supplicant/src/rsn_supp/wpa.c#L1064)
also accepts all four IDs. `GroupKeyId::select` addresses four protocol
records; physical slots come from the backend's pool. The wire ID does not
prescribe a hardware slot layout or narrow the portable WPA2 receive contract. Legacy WPA/TKIP and mesh
key-selection policies are outside this station's supported WPA2-CCMP mode.

`poll_action` drains internal transitions until it produces an action or
reaches an external wait. The Associated action is a join step, delivered
before Connected or handshake work. Disconnect waits for an absence to end
before deauthentication, and waits for TX credit instead of discarding that
frame. The bounded transition graph is documented beside `poll_action`.
An unsuccessful or refused scan follows the same Leave path and disables
reception before its terminal notice. A lease terminal received during Leave
preserves pending key removals, deauthentication and configuration; it ends
Leave only when the machine already waits for that lease terminal.

Every `StaTransmit` or `DataTransmit` carries an owned context alongside its
buffer and ticket. The service puts that context in the port attempt and
returns it through `TxDone` or the local unsent path. A station context
names the connection epoch and purpose (authentication, handshake message,
PM transition, probe or rekey); the current state never guesses which
operation a terminal ended. Connected control admission remains one at a
time; its flags describe pending policy transitions, not completion identity.
The complete `SingleOutcome` reaches the station's state transition. Power save
then interprets a Null's outcome as Delivered or Unconfirmed, keeping one of
three states: Awake, Asleep or AwakeUnconfirmed. An unconfirmed PM=0 only
needs reconciliation through TIM when an earlier PM=1 may have reached the
access point. The Null retry budget is a `NonZeroU8` from its constant
definition through the service's retry-plan limit.

A data context keeps the link epoch, original TID, order, checked
`SizedFrame<AckHeader, K>`, access category and protection. The immutable
frame description preserves its header/length proof through an unpublished
return; no constructor or frame-byte inspection runs again. `DataTransmit`
has closed fields and consumes into context, ticket and buffer. Frame and
access category occur only in the context; the admission permission holds
only its pool bit. `DataProtection` records Open or CCMP at encoding and
borrows the matching link's key at submission. A changed epoch or protection
returns `TxRefusal::LinkChanged` with every resource, before preparation;
a protected encoding cannot fall through to an open transmission. A submitted
context owns an `AcPermit` from the pool sized by backend queue depth
(and bounded by the tickets granted to Data). Returning the context returns
that permission; there is no `at_port` counter, in-flight metadata FIFO or
TID decoding at completion. Voice keeps the same data depth bound, so a
station operation waits for at most that depth plus one on-air attempt. An unpublished return is inserted by its original order,
so a batch of returns preserves both per-TID and shared-AC order. An old
link's terminal returns its tokens without requeueing or affecting the new
link; the service checks the same epoch before updating rate selection.
`LinkView` still borrows sequence/replay/key state only for a synchronous
step. No queued context borrows the station or data machine.

`Station::receive` admits Data and EAPOL together through MAC's checked
`DecodedHeader` and `AdmittedData`. The header owns the MAC/QoS/HT boundary;
CCMP admission requires integrity evidence and commits replay once before
EtherType dispatch. The service delivers non-EAPOL MSDUs from this same
admission, counts checked protocol drops and reports `RxFrameDropped`.
Malformed A-MSDU subframes retain their error instead of disappearing from
an iterator.

M3 and protected GM1 authenticate before entering the exchange. Each kind has
one affine deferred-frame owner (`PendingM3`, `PendingGroupM1`); a newer valid
counter replaces its predecessor. Invalid MICs and counters no greater than
the processed or same-kind deferred frontier are counted protocol drops, not
lifecycle refusals. M3 and GM1 share one replay frontier; deferred frames
run in ascending counter order at successive M4/GM2 TX terminals, whatever
their delivery outcomes. An exact completed M3 still
coalesces a cached M4 without reinstalling PTK. Every M4 mirrors the received
M3's MAC protection; an open M3 produces an open M4 even with an installed PTK.

The installed-key phase before connection uses the same group-only exchange,
under the original D4 M1/M3 deadline. Only a delivered M4 opens application
data. Replacing a deferred frame never extends that deadline. Connected has
no rekey or repeated-M3 timeout: the port guarantees a finite TX terminal,
including watchdog Indeterminate. The AP ends an unsuccessful exchange by
deauthentication. Disabled enters Leave and returns both deferred owners in
`RetiredStation::into_outputs`, together with the key and frame resources;
no cleanup operations are sent to a disabled port. The rustdoc of
[`deferred_eapol`](src/machine/deferred_eapol.rs) links the pinned vendor replay,
group-exchange and TX-callback paths and its empty auth-timeout cancellation.

Station alone issues `LinkEpoch`, before starting Connect. Its non-cloneable
`LinkEpochSource` consumes the install's unique `Slot<ConnectionEpochKind>`
from `oer-ieee80211-port`; there is no reset or slot extraction. The
backend supplies that slot, while Station constructs and owns its source. The epoch and contexts carry the invariant
install brand. Recreating Station therefore cannot restart its counter
while earlier operations can return. Issuance is total with a wrapping `u64`.
An outstanding operation must not survive an entire cycle of 2^64 connections;
this lifecycle condition is documented on `LinkEpochSource::begin`. The
ticket bound limits concurrent operations, not their age. Ending a link
closes its epoch without allocating another. Data takes this same epoch
from `LinkView`; temporary sleep or a channel absence does not close it.

Station and Data construct `ClassSizedFrame` while encoding into their class's
buffer, then retain that proof through the operation. The service only adds
the phase's rates; it cannot replace the checked header or length.
`TxRefusal::NoRate` belongs to service selection, while
`TxRefusal::Preparation` carries a pool refusal. Neither is a radio terminal.

Preparation refusal is `StaInput::Unsent { reason, .. }`, distinct from
`TxDone`. Refusing M2/M4 ends the handshake through key removal and the
normal leave path; an unsubmitted M4 cannot establish a connection. A
required join frame refused at preparation ends that join. Connected PM
frames retain the existing unsuccessful-send policy. An unsubmitted connected
group response releases its stage and permits the deferred successor without
a local disconnect.
An admitted M4 establishes Connected only on `Ended(Delivered)`. Other
radio outcomes return their ticket and buffer and wait for another M3 under
the original handshake deadline; Disabled starts Leave. There is no automatic
M4 retry after an indeterminate result. A repeated authenticated M3 may advance
its replay counter and group material, while the installed PTK and its replay
frontier remain intact. Unchanged GTK material is acknowledged without another
installation; changed GTK material follows the same install/replace terminal
rules as connected rekeying.
Data's `DataReturn::Refused` ends that MSDU and releases its permission and
tokens once, without retrying unchanged invalid input. The service reports
the typed reason to its application and continues; preparation refusals
never enter radio/rate statistics.

`StationResources` groups the epoch source, ticket and small-buffer pools,
channel places and key slots transferred into Station. Completion inputs
carry `ReturnedMpdu::Fresh` or `Sent`; service preserves that state. Only
the machine renews a published MPDU when retiring it, releasing its body
and returning fresh memory to its pool.

`StaData::returned` consumes a complete terminal or preparation refusal and
has no `Result`. The service exhaustively pairs `TxContext` with the closed
Small/Large fresh-or-sent buffer enum. A mismatched pair or an aggregate
terminal for this singles-only service is a backend contract violation:
it takes the Poisoned fail-stop path without a stray-terminal counter or
resource reuse. `Indeterminate` retires and counts the MSDU; Station treats
it as unconfirmed. Neither it nor `ContentionLimit` trains rate selection.

A gate withdrawal retains the MPDU at its original TID/order. Fresh memory
is prepared normally; Sent goes through retransmission with the same SN/PN.
KeyRemoved retires that MSDU and releases admission and resources. A barrier
waits only the current publication; rate selection ignores every withdrawal.


## Design decisions

- **Modem sleep.** Split first: `Connected` keeps only the deciding half, as
  a typed state. Null-frame retries belong to the driver's attempt plan; the
  coexistence part belongs to the radio system.
- **Link loss.** The beacon-loss window pauses during an absence and restarts
  on `LeaseEnded`. In power save it counts expected beacons (from the listen
  interval or DTIM). `StaBeaconLossConfig` and `StaLinkProbePolicy` live in
  `StaConfig` without defaults; the composition supplies the values with
  their `SOURCE`.
- **Pattern A, answer slots.** An `AnswerSlot` is taken for each request.
  Every exit from `AwaitAnswer` carries the slot into an explicit state.
  Cancelling while a request is in flight is legal. `UncorrelatedAnswer` is
  an error only when no slot exists.
- **Pattern B, refusals.** Each command kind has its own refusal.
- **Pattern E, bounded steps.** A `while … take_due` loop has a step limit;
  exceeding it is a contract error. An absence ends no later than
  `listen_end`.

## Responses to lifecycle actions

`Station` owns keys and free slots across scan/join/handshake/leave
transitions. The pairwise cell is Free, Installing, Installed or Removing.
Group slots are fungible credits from the composition's declared pool;
installed owners occupy private reserved records in installation order. A
pending install owns a vacant record; returning it cannot overflow a pool
of retiring keys. With a free physical slot the new GTK installs there;
previous installations and their replay frontiers remain until eviction or
Leave. A full pool uses `replace_group_key`, evicting the oldest non-current
installation. The same algorithm applies to every capacity: if the only
installed key is current, replacement consumes it atomically. Selecting
retained material preserves its installation age and replay frontier.
Leave removes all installed owners, with each terminal returning the
permission for the next removal.

A refused key-bank reunion retains the partial collection and every remaining
slot in quarantine. `Station::poll_action` returns the existing
`ContractViolation(IncompleteBank)` action, so the service follows its normal
contract-fault path without discarding or reusing any slot.

A correctly authenticated repeated M3, including a larger replay counter,
retains PTK. Its GTK follows that same material/slot policy. `RsnStaRetryInstall`
contains only group work, so a completed handshake cannot grant another PTK
installation. M4 retains the MAC protection of the M3 it answers, including
an open M3 after PTK installation and a protected repeat after a failed M4.
The service reports authenticated-key refusals through `StaRxError::Security`.

The requested ESP-IDF revision
[`7b9cc1ac`, `wpa_supplicant_send_4_of_4`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/wpa_supplicant/src/rsn_supp/wpa.c#L1316)
copies the request's replay counter, constructs the EAPOL MIC and passes the
response to `wpa_eapol_key_send`; that function hands an Ethernet payload to
`wpa_sm_ether_send`. Those functions do not select the 802.11 Protected bit.
The MAC protection rule here therefore follows an explicit project
requirement rather than an inferred meaning of EAPOL's Secure bit. The same
vendor file's `wpa_supplicant_install_ptk` at line 834 guards against repeated PTK installs.

Key actions and pending/installed GTK records keep material in the shared
`oer_ieee80211_wire::ccmp::Ccmp128Key` owner. Its Debug is redacted and its destructor zeroizes the
owned bytes when a record retires or an unfinished action is dropped.
Material comparison borrows those bytes; this makes no claim about
arbitrary compiler temporaries or copies made inside hardware adapters.

Each station receives one non-cloneable GTK mutation permission. Install,
replace, select and remove all consume it, so a pending operation excludes
another mutation without blocking RX. Each call reserves its FIFO terminal
and wakes the backend. The backend's `GROUP_COMMAND_BOUND` covers admission
through success or Poisoned, including the finite DMA/ISR/software RX cut.
Normal disable completes accepted commands within that bound. The interface
exposes no cancellation command; uncertain work remains owned for teardown.

The service retains the private pending action and matches its terminal by
physical slot. It never fabricates `GroupKeyInstalled` at command submission.
Until the terminal RX updates the old epoch. New material starts at KeyRSC
there, even below the old frontier. Selecting retained identical material
consumes its concrete installed owner and restores that record's live replay
frontiers; it performs no material write or counter initialization. Repeating
the current material leaves its installation and frontiers unchanged.

Exactly one installation of each association/wire KeyID can decrypt; other
IDs can remain active. Retained records of the same ID keep their own replay
state outside the active RX epoch. M4, group response and retirement resume
only after the matching terminal. Selection does not change physical
installation age. Every removal keeps its record place and material until
the terminal returns the slot and permission. Pairwise removal can overlap
a group mutation; two group mutations cannot overlap.

Hardware rejection poisons the port; internal rollback is not a success
terminal. The model and shared conformance exercise delayed ingress and
fault/disable custody. Each hardware backend must independently prove its
RX boundary. An evicted installation cannot be selected from a RAM material
copy.

Installation actions carry private pending tokens naming their cell, key ID
and link epoch. An old epoch's terminal returns its resources for normal
removal. Its private pending token is invariant in the port installation and
the station's epoch pool path, so another station's token cannot free a local
record or erase its replay frontier. It remains valid after the connection
leaves; completion does not require a current link epoch. Leaving waits for
all slots and the GTK permission before reporting.

`Configure` carries `PendingConfiguration`. Station grants only one such
token until it returns in `Configured`; an old epoch's reply releases that
permission without completing the new transition. The service keeps only
this outstanding action token. `StaTokens::GatePath` carries the channel
arbiter's component independently of `EpochPath`. Lease places, leases and
arbiter responses keep that path and the invariant installation lifetime;
neither another installation nor another gate in this one may answer them.
Arbiter responses are proved by
affine resources: Answer returns LeasePlace or Lease, AbsenceAnswer returns
AbsencePlace or Lease, and Exchanged consumes the old Lease and returns its
replacement. The service returns these synchronous answers in the same step.
None of these normal lifecycle paths adds fail-stop behavior.

A missing pending token is also a compile-time error. Owning rustdoc
supplies that compile-fail test and its valid counterpart; module
tests exercise stale handshake/rekey replies, slot return order and stale
configuration replies.

## Known defects

**Behaviour**

- **PMKSA never forgotten.** `machine.rs::Station::begin_authentication`
  only resumes and inserts; nothing calls `pmksa.rs::StaPmksaCache::remove`.
  After an AP restart or cache expiry, status 53 repeats on every Connect
  instead of falling back to full SAE.
- **No link-loss detection.** `link_monitor.rs::StaLinkMonitor` exists but
  nothing uses it; `StaConfig` has no beacon-loss values and there is no
  `BeaconLoss` reason. A lost AP is never noticed.
- **M4 is confirmed only by `Ended(Delivered)`.** `machine.rs::confirming_step`
  does not take the first verified CCMP frame under the new PTK as
  confirmation; `Station::link` is `None` in `Confirming`, so protected data
  received meanwhile is dropped as `NotConnected`, and `leave_handshake`
  sends a protected Deauthentication before confirmation.
- **Unprotected Deauthentication/Disassociation before PTK install is
  ignored** on a PMF link (`machine.rs::received_management`, the
  `Handshake` arm). The station stays in a dead handshake.
- **`Moved` and `Exchanged(Err)` are ignored in `Confirming`**
  (`machine.rs`, `StationNotice::Moved` and `StaInput::Exchanged`): only
  `Connected` reacts.
- **AP-initiated PTK rekey is not supported** (`machine.rs::ConnectedRsn::receive`
  answers it `UnsupportedMessage`). A recorded capability gap, not planned.
- **Modem sleep is not wired.** `modem_sleep.rs::ModemSleep` (bool fields,
  `Clone` as rollback, the coexistence slice inside) has no caller; only
  `power_save.rs` is in the machine. No power save on hardware.
- **Only `UpstreamFact::Connected` reaches the arbiter**; `SwitchAnnounced`,
  `Lost`, `Found`, `JoinFailed`, `NotFound` and `Purpose::Search` are never
  used, so the arbiter cannot follow a switch or a loss.

**Types and ownership**

- **Actions borrow `self`:** `machine.rs::StaAction` `RequestAbsence { home: &Lease }`,
  `InstallGroupKey { association: & }`, `TransmitProtected { key: & }`.
- **Arbiter answers are not correlated:** `machine.rs::answered`,
  `absence_answered` and `StaInput::Exchanged(Ok)` overwrite or drop a
  `Lease`/`AbsencePlace`; an owner can be lost silently.
- **Placeholder and flag state:** `State::Taken`, `KeyCell::Installing/Removing`
  (via `mem::replace`), `Option` fields `configuration`, `place`,
  `absence_place`, `absence`, `epoch` on `Station`, `Held::Released`, bools
  and `Option`s in `Connected`.
- **`Commands::complete`** does nothing unless the cell is `Pending`; command
  credits prevent a second request from overwriting that cell or its terminal.
- **`machine.rs::Owed` variants carry no proof**; they re-read `self.lease`
  and `self.keys.pairwise` behind `_ =>` arms.
- **Silent losses:** `pool.rs::Pool::put` drops and counts; public
  `Station::lost_tokens`; unreachable `data.rs` `overflowed` sites.
- **Authentication attempts matched by number:**
  `machine.rs::TxPurpose::Authentication { attempt: u16 }`, with
  `join/authentication.rs::begin_attempt` using `saturating_add`.
- **`machine.rs::Station::finish` drops `lease` and `absence`** instead of
  returning them to the arbiter.
- **Low:** catch-all `_ =>` / `state => state` arms in `machine.rs`
  (including `_ => AuthenticationFailed`); `machine/management.rs::receive`
  masks raw frame-control bits and silently ignores a frame over 96 bytes;
  `SaQueryStep::TimedOut` is discarded; raw `u16` reason codes (no reason 17
  for an M3 RSN mismatch; `leave_handshake` derives a local reason from the
  wire code); `pub` fields on `StaTransmit`, `LinkView`,
  `StaCommandTerminal`, `StaSaeTransmission`, `StaSaePmk`; `Unsent` mapped
  to `NullOutcome::Unconfirmed`; `data.rs::DataRefusal::NoTokens` without a
  typed wake; one EAPOL error aborts the rest of an A-MSDU; dead
  `Unsent::Encode` arms.

**Security (A-MSDU and keys)**

- **Non-constant-time comparison** of GTK/IGTK material in
  `machine/group_keys.rs` `current_material` and `take_matching`.
