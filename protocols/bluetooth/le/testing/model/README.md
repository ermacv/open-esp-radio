# Bluetooth LE model port

`oer-protocols-bluetooth-le-testing-model` is a host model of hardware
behind the Bluetooth LE port's contract
(`oer-protocols-bluetooth-le-contracts-port`): `ModelPort` implements
`LePort` and its round's parts, and is the conformance suite's subject. No
product package depends on it.

- **A scheduler** with one list ordered by start and a clock the test moves
  (`advance`). An event published later than its start less `MODEL_GUARD`
  ends `NotExecuted(Late)`; an event the test displaces (`displace`) ends
  `NotExecuted(Displaced)` at its start; every other event begins at its
  window's start and ends at its window's end.
- **An air with a scripted peer** a role (`script`, one `Turn` an event):
  a scanner's or an initiator's request to an advertising set, the
  advertisers a scan window hears, a central that sends, acknowledges,
  withholds its acknowledgement or is lost, a tester's packets.
- **A connection's transmit chain and receive nodes** by the geometry: `RX`
  nodes a connection and for each of the two shared chains (advertising
  and scanning). A PDU of the chain is `Transmitted` in the event whose
  central acknowledged it; with no node free the central's PDU is neither
  taken nor acknowledged and is sent again.
- **A withdraw** ends what a role has out: an event not begun
  `NotExecuted(Withdrawn)`, one begun runs to its end, every PDU of the
  chain `Withdrawn`.
- **Anomalies** (`raise`): the port needs a stop; until the stop nothing
  runs and a round gives nothing.
- **A stop** by the contract's one path: the stop's request, then the
  blocks' reset, which ends every event out (`NotExecuted(Withdrawn)` if
  not begun, `Indeterminate` if begun) and every PDU in a chain
  (`Withdrawn`) through the rounds that follow. There is no wait. After the
  reset the hardware is as the install found it: a start is cold.
- **A record** of each run (`runs`): what the model was asked to program
  and what it answered, instants as offsets from the run's start, so that
  a second run's record equals the first's.
- **Counts** of its calls (`counts`).

The crate's documentation names what it assumes where the stand has not
answered (the contract's open points B4, B5, B6, B10, B11), what it does
with a round dropped with no close, and with receptions not read.
