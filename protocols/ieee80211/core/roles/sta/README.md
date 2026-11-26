# Portable Wi-Fi station policy

This crate contains the station, independent of a chip, executor and network
stack. It is sans-IO: `machine` is one state machine for the whole lifecycle
(scan, SAE or Open System join, RSN four-way handshake, connected, background scan, group
rekey, leave) that takes the port's events, the channel arbiter's answers and
notices, commands and time as values and leaves the port's and the arbiter's
actions as values. A thin service loop over the lower-MAC port feeds it.

Produced actions and transmissions are `must_use`: the consumer performs or
forwards them before waiting again. The station holds nothing the port has a
limited number of. Its inputs carry outcomes and its actions carry values; it
is generic over the types of those values (`StaTypes`: the arbiter's path,
the brand of its command credits and the port's evidence of a frame).

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

After the port's stop, `Station::finish` ends the run in place: every
accepted request ends, the station's channel places go back to the arbiter
(`StationPlaceReturns`) and the station owes no configuration, key removal or
frame to the stopped port. A frame out at the finish keeps its place until
its end arrives, across a restart. `Station::poll_output` then gives the requests'
ends and the one lifecycle notice, and `Station::take_deferred` the frames
its RSN deferred. The stopped arbiter consumes the places and the cancelled
lease returns, then hands out the complete `StationPlaces`;
`Station::restart` takes them, keeps the profile, the configuration and the
keys' progress, and resets the rest in place. The restarted station removes
the keys and the receive configuration its last run left before it takes a
Connect.

The station keeps no diagnostic counters and holds no sink. The calls that
can lose or drop something (`Station::input`, `receive`, `poll_action` and
`try_block_ack_reply`) take the caller's `&mut impl EventSink<StaEvent>`.
They report a group table place returned to its full pool (`PlaceLost`), an RSN
input dropped without a returned error (`RsnInputDropped`: key data that did
not unwrap, fit or install, or a refused unwrap result) and each beacon or
probe response the scan table took (`ScanBssObserved`), a connect's step
(`ConnectStep`, sent at each entry for a sink to keep the changes) and each
frame the scan heard (`ScanHeard`), values of
`events::StaEvent`. A refusal a call returns, such as an EAPOL frame's
`StaRxError` or a preparation refusal the service passes to `returned`, is
its caller's to count and is no event. The scan result keeps its typed
completeness (`ScanSummary::completeness`: `Complete`, or `Truncated` when a
new BSS found the table full); how many frames or BSSs a scan heard are
events only.

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

The station's security policy lives in `security.rs`: `StaSecurityPolicy`
names what one request admits, `matches_security` admits a scanned BSS for
one exact mode and `select_association_rsn` chooses the AKM and management
protection; `personal_rsn` chooses the RSN capabilities (SPP A-MSDU capable,
MFPC under management protection, MFPR and the H2E RSNXE under SAE), whose
RSN element and RSNXE `oer-protocols-ieee80211-wire` encodes (`station::SelectedRsn::rsn`).
The scan table keeps each observation's signal level (`None` when the
receiver reported none, ranked below every known level) and reception time
beside its decoded `ScanRecord` (`scan::BssObservation`). A connect's scan
admits a BSS into its table (16 records) only when it names the profile's
SSID, so other networks heard on earlier channels never crowd out its own;
a held record is updated by any frame of its BSSID. A background scan
records every BSS.

SAE observations consume the exchange and return it only while it continues.
Commit and confirm have checked absolute deadlines, including time waiting
for the SAE frame out to end. An anti-clogging token repeats the same commit
without extending its deadline. RX may advance before the outgoing frame's
end; that end cannot restart an earlier exchange. Timeout, peer refusal and
invalid confirmation retain their concrete failure causes.

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
A network's profile holds what the station asks its access points for
(`StaProfile::phy`, an `association::StaPhyRequest<K>`): the set of formats
it admits and its widest width, checked against the chip's `K::CAPS` when it
is made, in a const item where it is a constant. An empty set, or HE at a
width the chip does not offer, does not build (`tests/ui/phy_request.rs`).
The format is decided once, when the candidate is chosen from the scan
(`association::negotiate`): the highest format the request admits and the
access point offers, HE first (`association::he_admitted`: HT Capabilities,
HE-MCS 0..9 both ways on one stream and an HE Operation that parses), then
HT, then non-HT; and only then the width, 40 MHz where the request's
ceiling, HT's widths and the access point's secondary channel allow it. An
access point offering no admitted format is no candidate: if no access
point of the network offers one, the join ends with `NoAdmittedFormat`
before any frame. The decision (`NegotiatedPhy`) goes with the join to the
request's encoder, which cannot refuse it. The HE Power Capability and HE
UL-MU power elements are the scanned channel's (`ScanChannel::he_power`):
the composition computes one value a channel from its transmit ceiling when
it builds the configuration, and an HE request carries its channel's. They
are the scanned primary's because HE is offered at 20 MHz only. An HE association takes
its HE facts (capabilities, peer state, BSS color) from the Association
Response's HE Capabilities and HE Operation, which must parse and still admit
HE20; the scan's elements chose the request only. A response that lacks,
repeats or malforms an HE element, or no longer admits HE20, makes no link:
the station leaves with `HeNotGranted` and never keeps an HT link in its
place; so does a response whose AID or HE parameters lie outside the port's
`HeAssociation`. The connection notice and the Connect outcome name the PHY
mode the station associated in. The port's HE state follows the pinned
vendor station's two stages (`StaAction::SetHe`): before authentication
(and before the BSS's receive configuration) an HE20 join sets the HE BSS
with the color the scan heard and any other join sets `StationHe::Off`;
after the Association Response, its EDCA parameters and before the link
starts (the 4-way handshake's frames included) an HE20 association sets
`StationHe::He`. The leave of an HE BSS sets `Off` before its receive-off
configuration. A restart runs on the next installation, whose MAC starts
without an HE association.
Authentication frame tags also retain the attempt ordinal beside their link
epoch. A retry waits for the earlier attempt's frame to end; that end, or its
refusal, cannot end the retry or arm the retry's response timer. RX may still
complete the current exchange before its frame's end arrives.

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

- `machine`: the station, the types of its values (`StaTypes`) and its host slice tests
  beside the real arbiter;
- `machine::block_ack`: the machine validates association addresses and PMF
  before handing over Block Ack management (`PendingRxBlockAck`), and sends
  replies as its connected station's frames (`RxBlockAckReply`); the recipient
  agreements themselves are `oer-protocols-ieee80211-core-upper-mac`'s `block_ack::banks`;
- `machine::tx_block_ack`: the station's transmit agreements
  (`StaTxBlockAck`), one `TxBlockAckOriginator` of the eight TIDs that the
  composition builds from its `StaTxBlockAckPolicy` and hands over in
  `StaConfig::tx_block_ack` (`None` negotiates nothing). Each association
  starts it afresh and admits agreements only with an HT access point of a
  QoS BSS. Data of a TID (`Station::tx_traffic`, with the TID's next
  sequence number as the link's holder numbers it) queues that TID's
  negotiation; the station sends the ADDBA Request as a connected station's
  frame, from that next sequence number, and retries an unanswered or
  unsent request after the policy's interval while attempts remain. A
  refusal, spent attempts or a response outside the request (which owes a
  DELBA) close the TID for the association. The access point's DELBA as the
  recipient ends an agreement without an answer; an agreement idle for the
  policy's inactivity ends with the station's DELBA (reason 39), and new
  data negotiates again. Each change of an operational agreement goes to
  the link's holder (`LinkAction::Agreement`), whose window the data plane
  keeps from its starting sequence on; `Station::tx_block_ack_request` owes
  the BlockAckReq the plane names
  after a drop, owed again at that head when it goes unanswered. Other
  Block Ack actions stay with the receive agreements;
- `data`: the connected station's received data: the network boundary's
  checked `EthernetFrame` and the BSS's data MPDUs into MSDUs. No reorder,
  replay check or power save yet;
- `link`: the connected station's link as it hands it to the holder that
  carries its data (`LinkAction::HandOver` with a `LinkHandover`). The
  holder keeps the link's numbers (`TxNumbers`: each TID's QoS sequence
  space, the station's own space, which management frames and non-QoS
  data share; the holder adds its own pairwise key with its CCMP packet
  numbers, `LinkHandover::with_key`) and its receive admission (`RxLink`: own
  address, BSSID, protection and the replay epoch). It numbers the data
  and every station frame of the link (`link::number`,
  `FrameNumber::Link`: the station writes zero, `link::number` writes
  the Sequence Number into the frame's header as it is submitted; its
  packet number follows its rate), and admits received data without the
  station (`LinkHandover::receive`): data to the network, EAPOL to
  `Station::eapol`, management frames to `Station::receive`. The station
  holds and releases the link's data (`LinkAction::Hold`/`Release`: key
  confirmation, sleep, absences), tells each transmit agreement's change
  and asks for the link back once it leaves (`LinkAction::Return`); the
  holder returns it (`LinkReturn`, `Station::link_returned`), which the
  station numbers on from. `Station::poll_link` gives the hand-over, hold,
  release and return the holder applies right after an input; every
  change also comes first from `Station::poll_action`. The pairwise key is
  the engine's: the station removes it only after the link came back. A
  group key's installation, replacement, selection or removal end switches
  the replay lane in the link (`LinkHandover::group_terminal`, from the
  station's own record of the operation) before the station takes it; the
  station switches it itself only before its link exists. The service
  returns the link before the station finishes;
- the station's data transmission (the data plane over the port's two
  arenas), the link's numbers (`TxNumbers`) and the MAC's transmit queues
  are `oer-protocols-ieee80211-core-datapath`'s;
- the transmit decisions of a station frame travel with its action:
  `StaRatePlan` selects its rate record and transmissions, `StaFrameClass`
  its coexistence class, and `StaTransmit` its access category (Voice), its
  protection exchange (none) and its calibrated power; the service carries
  them into the port's attempt unchanged;
- `join`: Open System and SAE authentication and Association state machines
  and their retry policy;
- `association`: association capability selection from the scan record;
- `power_save`: what the access point knows of the station's power state and
  the TIM's decision;
- `station`: the application's vocabulary (lifecycle stages, fault reasons,
  reconnect policy);
- `sa_query`: the SA Query procedure of an association under management
  frame protection;
- `link_monitor`: beacon-loss decisions;
- the station's SAE PMKSA cache, `oer_protocols_ieee80211_core_security_rsn::pmksa::PmksaCache` keyed
  by BSSID and SSID, owned by the station across connections;
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

The profile's SSID and SAE bodies own their bounded lengths in heapless
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

`Station::new` (and `init` in place) takes the station's two places with the
channel arbiter (`StationResources`) and returns the machine and its three
command credits. Its `GroupCapacity<GROUPS>` proof names how many group keys
the port holds at once; a `const` item evaluates it, and a zero capacity
fails the build, since any later profile may protect its link.

The station has no network until `Station::set_profile` gives it a
`StaProfile`: the SSID and `StaSecurity`. Connect without one returns
`CommandRefusal::NoProfile`. Only an idle station takes a profile; otherwise
`StaProfileRefused` returns the same profile with the refusal a Connect would
get (`AlreadyConnected` or `Transitioning`). The profile stays for every later
Connect until the next replaces it; the previous profile's PMK and SAE
password erase on drop, and the PMKSA cache forgets that SSID's
associations. Every connection phase reads the profile its Connect was
admitted with.

The station encodes every frame into one buffer of its own
(`STA_FRAME_CAPACITY` bytes) and lends it to the action that carries the
frame out in the same step (`StaTransmit`). It has one place for each purpose
of its frames: probe, authentication, SAE authentication, association,
handshake response, handshake last, deauthentication, and the connected
station's frames, which go one at a time. A frame waits while the frame of
its own purpose is out, from its action to its end (`StaInput::FrameEnded`),
never for another purpose's. Authentication and Association take their
place before beginning a request or consuming its sequence number;
Authentication's retry becomes an active exchange only then, and a beacon,
Probe Response or Authentication Response while it waits cannot finish an
exchange that has not begun. EAPOL (including the QoS rekey response), Null,
deauthentication and Probe take their place before consuming a sequence.
Association starts its deadline when entered, before its first
transmission; its overdue retry time does not cause repeated polling. Every
four-way handshake step retains the same response deadline while waiting for
its frame, a key or its installation. Expiry leaves once. After taking its
place, encoding can fail only with `EncodeError`; an encoding failure leaves
once, and a missing connection is reported as a changed link.

GTK wire IDs include zero. IEEE 802.11-2020, 12.7.2 (EAPOL-Key frames),
defines the GTK KDE's two-bit KeyID. Frame addressing distinguishes PTK
and GTK. The pinned
[ESP-IDF parser](https://github.com/espressif/esp-idf/blob/4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d/components/wpa_supplicant/src/rsn_supp/wpa.c#L1064)
also accepts all four IDs. `GroupKeyId::select` addresses four protocol
records; the station's group table names a key by its place
(`StaGroupPlace`), which the engine maps onto its hardware. The wire ID does not
prescribe a hardware slot layout or narrow the portable WPA2 receive contract. Legacy WPA/TKIP and mesh
key-selection policies are outside this station's supported WPA2-CCMP mode.

`poll_action` drains internal transitions until it produces an action or
reaches an external wait. The Associated action is a join step, delivered
before Connected or handshake work. Disconnect waits for an absence to end
before deauthentication, and waits for a Deauthentication still out instead
of discarding the frame. The bounded transition graph is documented beside `poll_action`.
An unsuccessful or refused scan follows the same Leave path and disables
reception before its terminal notice. A lease terminal received during Leave
preserves pending key removals, deauthentication and configuration; it ends
Leave only when the machine already waits for that lease terminal.

Every `StaTransmit` carries a tag (`StaTxTag`): the connection epoch and the
frame's purpose (authentication, handshake message, PM transition, probe or
rekey). Its end comes back with that tag, sent or refused
(`StaInput::FrameEnded`); the current state never guesses which operation an
end ended. Connected control admission remains one at a
time; its flags describe pending policy transitions, not completion identity.
The complete `SingleOutcome` reaches the station's state transition. Power save
then interprets a Null's outcome as Delivered or Unconfirmed, keeping one of
three states: Awake, Asleep or AwakeUnconfirmed. An unconfirmed PM=0 only
needs reconciliation through TIM when an earlier PM=1 may have reached the
access point. The Null retry budget is a `NonZeroU8` from its constant
definition through the service's retry-plan limit.

The link's holder admits a connected station's Data and EAPOL together
(`LinkHandover::receive`) through MAC's checked `DecodedHeader` and
`AdmittedData`; before the link exists, `Station::receive` admits the
handshake's EAPOL the same way and refuses its data. The header owns the
MAC/QoS/HT boundary; CCMP admission requires integrity evidence and commits
replay once before EtherType dispatch, also while the link holds its data.
The service delivers non-EAPOL MSDUs from this same admission and reports
each checked protocol drop as an `RxFrameDropped` observation with its
reason.
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

Every AES operation of the station runs in the step over the synchronous
AES-128 engine the composition supplies through its callbacks
(`StaCallbacks::AesEngine`, lent by `aes_engine`): the key-data unwrap
(RFC 3394, `oer-portable-crypto`'s `rfc3394::unwrap`), Message 3's when the
handshake takes it and a connected rekey's in the action turn that follows;
the AES-CMAC EAPOL-Key MIC of PSK-SHA256 and SAE; and the CCMP and BIP of
received protected management frames. A refusal drops such a frame like one
that does not verify and leaves its replay state unchanged. An engine held in
the station's callbacks is debt by the rule of
[`docs/architecture.md`, "Ownership"](../../../../../docs/architecture.md#ownership),
removed when the roles are written (the plan's step "the radio's controller
and the roles"). The machine stays sans-IO: it names
no hardware type, and a composition may supply a software block or a chip's
accelerator behind the same trait. An engine's refusal fails the unwrap like
key data that fails its integrity check. No unwrap is in flight across
inputs.

The installed-key phase before connection uses the same group-only exchange,
under the original D4 M1/M3 deadline. Only a delivered M4 opens application
data. Replacing a deferred frame never extends that deadline. Connected has
no rekey or repeated-M3 timeout: the port guarantees a finite TX terminal,
including watchdog Indeterminate. The AP ends an unsuccessful exchange by
deauthentication. Disabled enters Leave; once the station finishes, it gives
both deferred owners back (`Station::take_deferred`), and no cleanup
operation is sent to a disabled port. The rustdoc of
[`deferred_eapol`](src/machine/deferred_eapol.rs) links the pinned vendor replay,
group-exchange and TX-callback paths and its empty auth-timeout cancellation.

Station alone issues `LinkEpoch`, before starting Connect: a value its own
counter makes, wrapping at `u64::MAX`. An old epoch equals a new one only
after 2^64 attempts while one of its frames or commands is still out. Ending
a link closes its epoch without issuing another. The link's holder takes this
same epoch with the link (`LinkHandover::epoch`); temporary sleep or a
channel absence does not close it.

Station constructs `ClassSizedFrame` while encoding into its own buffer
(`StaFrames`), then retains that proof through the operation. The service only adds
the port's checked rates; it cannot replace the checked header or length.
`TxRefusal::NoRate` belongs to service selection, while
`TxRefusal::Preparation` carries a pool refusal. Neither is a radio terminal.

Preparation refusal is a frame's end `StaFrameEnd::Refused`, distinct from
`StaFrameEnd::Sent`. Refusing M2/M4 ends the handshake through key removal and the
normal leave path; an unsubmitted M4 cannot establish a connection. A
required join frame refused at preparation ends that join. Connected PM
frames retain the existing unsuccessful-send policy. An unsubmitted connected
group response releases its stage and permits the deferred successor without
a local disconnect.
An admitted M4 establishes Connected only on `Ended(Delivered)`. Other
radio outcomes wait for another M3 under
the original handshake deadline; Disabled starts Leave. There is no automatic
M4 retry after an indeterminate result. A repeated authenticated M3 may advance
its replay counter and group material, while the installed PTK and its replay
frontier remain intact. Unchanged GTK material is acknowledged without another
installation; changed GTK material follows the same install/replace terminal
rules as connected rekeying.
`Indeterminate` ends the frame; Station treats it as unconfirmed. Neither it
nor `ContentionLimit` trains rate selection.

A barrier waits only the current publication; rate selection ignores every
withdrawal.


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

`Station` keeps each key's progress across scan/join/handshake/leave
transitions: the pairwise key is none, installing, installed or removing.
The group table holds as many keys as `GROUPS` names, in installation order;
one group operation runs at a time (install, replace, select, remove), and
the station owes the next only after its end. A pending install reserves a
place. With a free place the new GTK installs there; previous installations
and their replay frontiers remain until eviction or Leave. A full table
replaces the oldest non-current installation. The same algorithm applies to
every capacity: if the only installed key is current, replacement consumes it
atomically. Selecting retained material preserves its installation age and
replay frontier. Leave removes every group key, one at a time, then the
pairwise key once the group table is empty and the link came back.

A correctly authenticated repeated M3, including a larger replay counter,
retains PTK. Its GTK follows that same material/slot policy. The connected
supplicant answers it with only group work, so a completed handshake cannot
grant another PTK installation. M4 retains the MAC protection of the M3 it answers, including
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
`oer_protocols_ieee80211_contracts_security::Ccmp128Key` owner. Its Debug is redacted and its destructor zeroizes the
owned bytes when a record retires or an unfinished action is dropped.
Material comparison borrows those bytes; this makes no claim about
arbitrary compiler temporaries or copies made inside hardware adapters.

One group operation in progress excludes another mutation without blocking
RX. Its end comes back with its place and its outcome
(`StaInput::GroupKeyEnded`); the station keeps the operation's record (key
ID, material, replay lane) and matches the end against it. Until that end RX
updates the old epoch.

A key operation's end carries `StaKeyOutcome`: done, or expired at its
bound. An installation, replacement or selection that expired ends its
connection attempt as a failed handshake (`HandshakeFailed`): the station
holds no key it asked for there (a selected installation stays, not
current), the link's replay lanes stay as they were, and the leave removes
what remains. A removal that expired ends as a done one: the station holds
the key no longer, so a leave or a restart's reclaim never waits for it
again. New material starts at KeyRSC
there, even below the old frontier. Selecting retained identical material
consumes its concrete installed owner and restores that record's live replay
frontiers; it performs no material write or counter initialization. Repeating
the current material leaves its installation and frontiers unchanged.

Exactly one installation of each association/wire KeyID can decrypt; other
IDs can remain active. Retained records of the same ID keep their own replay
state outside the active RX epoch. M4, group response and retirement resume
only after the matching terminal. Selection does not change physical
installation age. Every removal keeps its place and material until its end.
Two group operations cannot overlap. An evicted installation cannot be
selected from a RAM material copy.

An installation's record names its key ID and link epoch. An old epoch's end
leaves its key installed for the normal removal; it does not require a
current link epoch. Leaving waits for every key before reporting.

One receive configuration is set at a time (`StaAction::Configure`, then
`StaInput::Configured`); the station records the connection attempt it
belongs to, and an old attempt's end frees the configuration without
completing the new transition. `StaTypes::GatePath` carries the channel
arbiter's component. Lease places, leases and
arbiter responses keep that path and the invariant installation lifetime;
neither another installation nor another gate in this one may answer them.
Arbiter responses are proved by
affine resources: Answer returns LeasePlace or Lease, AbsenceAnswer returns
AbsencePlace or Lease, and Exchanged consumes the old Lease and returns its
replacement. The service returns these synchronous answers in the same step.
None of these normal lifecycle paths adds fail-stop behavior.

Module tests exercise stale handshake/rekey ends, the group table's places
and stale configuration ends.

## Known defects

**Behaviour**

- **PMKSA never forgotten.** `machine.rs::Station::begin_authentication`
  only resumes and inserts; nothing calls `PmksaCache::remove`.
  After an AP restart or cache expiry, status 53 repeats on every Connect
  instead of falling back to full SAE.
- **No link-loss detection.** `link_monitor.rs::StaLinkMonitor` exists but
  nothing uses it; `StaConfig` has no beacon-loss values and there is no
  `BeaconLoss` reason. A lost AP is never noticed.
- **M4 is confirmed only by `Ended(Delivered)`.** `machine.rs::confirming_step`
  does not take the first verified CCMP frame under the new PTK as
  confirmation; the link holds its data in `Confirming`, so protected data
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
- **No modem sleep.** Only `power_save.rs` is in the machine: the station
  tells its access point its power state, and never sleeps. No power save
  on hardware.
- **Only `UpstreamFact::Connected` reaches the arbiter**; `SwitchAnnounced`,
  `Lost`, `Found`, `JoinFailed`, `NotFound` and `Purpose::Search` are never
  used, so the arbiter cannot follow a switch or a loss.

**Types and ownership**

- **Actions borrow `self`:** `machine.rs::StaAction` `RequestAbsence { home: &Lease }`,
  and every `StaTransmit` lends the station's frame buffer.
- **Arbiter answers are not correlated:** `machine.rs::answered`,
  `absence_answered` and `StaInput::Exchanged(Ok)` overwrite or drop a
  `Lease`/`AbsencePlace`; an owner can be lost silently.
- **Placeholder and flag state:** `State::Taken`, `Option` fields `place`,
  `absence_place`, `absence`, `epoch` on `Station`, `Held::Released`, bools
  and `Option`s in `Connected`.
- **`Commands::complete`** does nothing unless the cell is `Pending`; command
  credits prevent a second request from overwriting that cell or its terminal.
- **`machine.rs::Owed` variants carry no proof**; they re-read `self.lease`
  and `self.keys` behind `_ =>` arms.
- **Silent losses:** `pool.rs::Pool::put` drops a place beyond a full pool
  and reports only a `PlaceLost` event; unreachable `data.rs`
  `overflowed` sites.
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
  wire code); `pub` fields on `StaTransmit`,
  `StaCommandTerminal`, `StaSaeTransmission`, `StaSaePmk`; `Unsent` mapped
  to `NullOutcome::Unconfirmed`; `data.rs::DataRefusal::NoTokens` without a
  typed wake; one EAPOL error aborts the rest of an A-MSDU; dead
  `Unsent::Encode` arms.

**Security (A-MSDU and keys)**

- **Non-constant-time comparison** of GTK/IGTK material in
  `machine/group_keys.rs` `current_material` and `take_matching`.
