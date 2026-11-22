# The receive ring on the stand

`oer-hil-probes-esp32s31-rx-ring` runs the HAL's receive ring
(`radio::ieee80211::dma::receive`) on the product's path, from the root's
claim, over the radio domain it holds (`tasks/277`, `281`). The bring-up and
the ring's start and stop are the path table's, in `src/radio.rs` of the
sender (`esp32s31-case-c-sender`), which this probe includes.

The ring:
- 16 descriptors, in an arena of 32 buffers of 256 octets with the
  64-octet header below the floor;
- the other 16 buffers are the spares a single is lent for;
- case C's 700- and 1 500-octet frames are chains of 3 and 7 descriptors.

A frame is the sender's when it is to the station, from the sender the host
named (`expect`), with Frame Control one of the cases'. It is then checked:
- every byte the sender sets agrees (Duration, Sequence Control and Retry
  are the MAC's);
- its length agrees with its case's: the bytes after the header are the
  MPDU without its FCS, rounded up to a word (`HW_RX_CHAIN_LAPS_20261117`).

The probe speaks the HIL protocol (`console = "hil"`): its console serves
the agent (`hil/agent`) and the receiver's requests
(`oer-hil-services-radio-pair`), each answered by what its hardware did.
The cases, their thresholds and the verdict are
[`hil/scenarios/radio-pair`](../../scenarios/radio-pair/README.md)'s.

| Request | What the probe does |
|---|---|
| `snapshot-cold` | before the bring-up: the reset cause, as the adoption read it, and the radio domain's power, clock and reset words (`RadioDomain::observe_power`: PMU, HP_SYS_CLKRST, and MODEM_SYSCON and MODEM_LPCON when the modem register bus reads open, unreadable otherwise) |
| `bring-up` | the PHY's registration, coexistence, power, the clocks, the PHY client, the tracking due, the MAC's cold start on the ring, the channel; then it reads its snapshot after the bring-up |
| `snapshot-up` | the cold snapshot's words read again after the bring-up, the temperature code, the RF PLL's frequency block and the PHY's calibration results |
| `receive-counters` | the MAC's receive counters, the walker's standing and every descriptor's control word, raw and as its fields read (armed, done with its `LENGTH`, other); no ring's state after a failed turn's stop |
| `expect` | the sender's address: a unit is the sender's when it is to the station from that address, never learned from a first frame |
| `window` | serves the ring for the given time: every single lent for a spare (copied in place when none is held), every chain copied in place, each checked; with chains recorded, each chain's descriptors and continuation words by position |
| `stall` | leaves the ring unserved for the given time, then reads where the walker stands |
| `turn` | a window, the stop (the walker's edge, the stop request and its hold, the reset after a stop, the release), the cold start, a window |
| `stop` | the stop; afterwards every step but the cold snapshot is refused until the next boot |

A step that fails answers its phase and index; the probe then refuses
every step but the snapshots, the counters and the stop (`StepFailed`)
until the next boot. The stop takes the hardware down as after a pass; a
turn that failed after its own stop has left the radio stopped, and the
stop is then refused as `Down`. A bring-up that fails leaves only the cold
snapshot.

The radio work is the radio level's one task (the interrupt executor of
level 2), private to the radio's owner, which runs on the application
level: an object the HIL console calls. What observes (the cold snapshot,
read at the composition) answers at once, also while a command works;
each command the owner hands to the radio level's task through the radio's hand-over (`src/radio.rs` of the
sender, shared by path) and answers with its result. Nothing of the HIL
agent is named on level 2 but the service's request and answer types as
data: the radio level's task refuses what it cannot do in its state by its
result, and the owner relays that answer.
