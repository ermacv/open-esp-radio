# Bluetooth LE port contract

`oer-protocols-bluetooth-le-contracts-port` is the boundary between a chip's
Bluetooth LE port and the sans-IO cores above it: the installation machine,
the controller with its roles and its plan of the air, and the ACL engine.
No backend implements it yet. No hardware, PHY membership, SRAM addresses or
HAL proofs appear in it.

## The port

`LePort` is one trait of the port's owner. No call answers an error; every
answer is an outcome, the one fact that the port needs a stop, `Pending` of
a poll or `Rest`.

- **The radio's life:** `start`, `poll_start`, `stop` (from `StopFrom`: a
  running radio or a start), `poll_stop`, `retire`, `release`. A stop always
  ends in `Stopped`; a start that fails answers `Stopped` beside its
  `StartFailure`.
- **The roles:** `open_*` and `close_*` for an advertising set, the scanner,
  a connection and the test session; `rewrite_advertising` between events;
  `write_accept_list` writes the whole list, which is the controller's state.
  A closing takes every place of its instance.
- **Events:** `publish_advertising`, `publish_scan`, `publish_connection`,
  `publish_test` take the event's place alone, which names its instance, and
  cannot be refused. The place comes back with the event's end in a round,
  whatever the end is (`EventEnd`: executed, not executed as late, withdrawn
  or displaced, or indeterminate). `withdraw` asks the hardware to give up
  what a role has out.
- **The transmit chain:** `write_pdu` fills a transmit place, `submit`
  appends it to its connection's chain; it comes back in a round as
  `Transmitted` or `Withdrawn`.
- **A round** (`Round`): what the hardware finished, read in one borrow of
  the port: `now`, the ended events (`Ended`, with their receptions as `Pdu`
  read in place), the transmit ends, held PDUs (`PduHandle`) read and
  released. A held PDU's bytes are read only inside a round. Its parts are
  `LeRounds`' methods, which take a seal only a round has.
- **Time and facts:** `alarm` arms the port's one alarm on the LE clock;
  `poll_facts` answers `Facts { finished, alarm, needs_stop }`.

## The rights

Every state a call needs is a right it takes (`rights`): a value of at most
two words, branded with the installation (`'id`), neither `Copy` nor
`Clone`, made only by this crate. The mint (`PortPools`, a pool set of
`oer-protocols-radio-port`) gives once the port's `PortKey`, `Stopped`,
`StopRight` and `RoleRights` (the empty places of every role and the accept
list's right); the port makes every next right from the one it was given
with its key. A role's right and its places carry their instance's index, an
event or transmit place also its slot's. `forge`, under `test-support`, makes
an installation's rights without a mint for the tests of the cores; no
product enables it.

## The geometry

`ADV` advertising sets, `SCAN` scanners, `CONN` connections of `EVENTS`
event places, `TX` transmit places and `RX` receive nodes each, and `TEST`
test sessions are the const generics of `LePort` and `PortPools`. The mint
and every round check them against the capabilities at build: `EVENTS` at
least 2, each count inside `LeLimits`, a scanner and a test session one at
most, and a role's count zero where the capabilities serve no such role.

## The vocabulary

- The clock domain: `LeRadio`, `LeInstant` and `LeWindow` on the LE radio's
  clock, which has no relation to monotonic time.
- The request values of every role: `AdvertisingSet` and `AdvertisingData`,
  `AdvertisingEvent` with its `ChannelSpacing`, `ScannerSetup`, `ScanWindow`,
  `ConnectionSetup`, `ConnectionEvent` with its `ConnectionPriority`,
  `dtm::DtmTest` and `TestWindow`, the coexistence `Operation` of an event,
  and regulatory-capped transmit power (`LeBand`, `LeTxCeiling`, `TxPower`).
- The capabilities (`LeCapabilities`, `LeRadioCapabilities`) and
  `Supported`, which checks a value against them when it is built. What the
  capabilities decide alone is a constant check of the value's kind; what
  depends on the host's input, a peer or time is checked by `new` and
  `placed`. A timed value is placed with `RadioTiming::reservation`, the one
  reservation function, within the capabilities' horizon.
- `RadioTiming` and `ConnectionAllowances`: the timing a backend reports.
- `PduLen`: a data channel PDU's payload length within the capabilities.
- `dtm::next_window`: the Direct Test Mode window arithmetic of Core Vol 6
  Part F. Transmitters keep the interval grid and skip unreachable slots;
  receivers keep their 5 ms windows.

`CipherOffload` is declared for a backend that deciphers in its hardware;
no backend implements it. A connection event's length bound is two values,
the event's window and its interval.

Wire PDUs, channels and addresses come from `oer-protocols-bluetooth-le-wire`.
