# IEEE 802.15.4 MAC engine

`oer-espressif-ieee802154-engine` is the MAC state machine and public API layer of the
public ESP-IDF IEEE 802.15.4 driver (`esp_ieee802154_dev.c` and its
helpers), ported once for every supported chip.

| Module | Contents |
| --- | --- |
| `engine` | Operation state machine, interrupt handler, receive ring state and statistics |
| `engine::owner` | The engine and its MAC backend held together, exposing semantic entries without a separate backend borrow |
| `pib` | The driver's PIB and its publication to the MAC |
| `ll` | The `Ieee802154LowLevel` interface (the driver's `ieee802154_ll_*` accessors, modem ETM steps and timer commands) and the hardware helpers generic over it; `ll::model` is a register model for tests and explicit `test-support` builds |
| `capabilities` | The portable capabilities a radio over the engine reports (ESP-IDF's OpenThread port defaults) and the table a chip builds from its transmit-power level set |
| `coex` | Operation-dependent software-coexistence policy and priorities |

All MAC semantic values come directly from
`oer-espressif-ieee802154-mac-values`: commands, timers, channel and power
values, ACK timeout, RX status, diagnostic counters, ETM, events, abort reasons
and Multi-PAN. Frame storage, its dimensions and opaque DMA address tables also belong to
mac-values. The engine does not re-export that vocabulary. Unnamed event
lanes 7 and 13 remain separate through fault capture and retained decoding.
`capabilities::portable_tx_power_levels` adapts the chip level set to the
portable protocol vocabulary. The engine retains its own FSM, environment
completion types, PIB, coexistence policy and storage/owner contracts.

Each chip's driver implements `Ieee802154LowLevel`; HAL converts semantic
values into its own PAC register types. The ESP32-S31 implementation is
`oer_esp32s31_ieee802154_driver::ll::S31Mac`. What differs between
chips stays with the chip: register geometry, the transmit-on delays and the
rest of the MAC lifecycle, the transmit-power level set, the coexistence
table and the diagnostic counters a chip's LL adds. Each HAL places its frame
storage with `dma_static!` in `.dma.bss.ieee802154`, consumes the `Placed`
claim and builds one typed address per RX, TX and enhanced-ACK slot. The
linker proves placement; the shared engine selects addresses from that table
without pointer arithmetic. Unknown model/hardware address identities match
no slot and are rejected.

The engine names no executor, interrupt route or chip. A runtime holds it
inside the chip driver's owner together with its MAC token and static buffers.
The owner's entries borrow its own MAC and expose frames and slot handles,
without DMA addresses or independently reusable register ownership. The
caller serializes entries and the interrupt handler in one critical section.
Its accessor sequence was compared on the host with the compiled vendor
driver.

## Configuration

Configuration is admitted in any radio state. The frame counter only moves
forward, except when it is replaced together with the keys. `pib()` inspects the retained
values; `update_pib(env, |pib| ...)` completes a synchronous engine entry. It
replaces values without queuing. The caller uses the raw engine's LL and
environment arguments or the owner's stored MAC.

RX values (promiscuous, coordinator, ACK flags and pending mode) publish at
once while the receiver listens without a frame in progress: stop RX,
publish the RX part of `pib.update`, restart RX. The hardware's
`is_current_rx_frame` after-SFD observation and latched frame events keep an
ongoing frame and its ACK intact. Changes during a frame, ACK transmission
or ACK wait publish at its terminal boundary, including automatic RX resumes.
An armed receive retains its timer and never starts early for a setting.
Channel, power and CCA publish at explicit operation start, including receive,
transmit, ED and CCA; automatic RX resumes publish only the RX part. Channel
is supplied with receive/transmit by the portable MAC.

Promiscuous reception acknowledges nothing: hardware ACK flags are the
configured flag AND NOT promiscuous. Disabling promiscuous restores each
configured flag, including changes made while promiscuous was enabled.

Addresses, ACK timeout and source-match edits take effect when their calls
return. The engine keeps PIB, all configured PAN identities and enable mask,
ACK timeout, source-match tables, receive-when-idle wishes and coexistence
across disable/enable. `mac_init()` republishes them after the MAC reset; PIB
defaults are chosen only at construction. Unset identity/timeout/mask fields
are captured from the first accessible hardware epoch (including a cold
policy); later epochs restore their shadow. The restore assumes every MAC
register was lost, so it also covers power-domain loss. The reset pulse
already precedes initialization in the S31 lifecycle. `ll::mac_init_registers`
sets event/abort enables, ED averaging and coexistence PTIs, leaving PIB and
identity restoration to the engine.

The host vendor catalog records three intentional differences: bounded RX
publication, promiscuous masking and retained settings. Each expectation belongs
to its scenario and the fingerprints of both complete boundary traces, including
every record after the first mismatch. A matching expectation reports `EXPECTED`
with that scenario's reason and succeeds; any other `DIFF` fails without expected
reasons. `compare all` succeeds when every scenario is `MATCH` or `EXPECTED`.
Tests also compare the remaining radio commands, aborts, buffers and callbacks,
and check the three differences independently against C and engine traces
(`port <scenario>` prints the latter).

## Bring-up and teardown order

The S31 diagnostic PHY image contains an RX-only assembly of the engine; its
current PHY first-client join rejects immediate tracking before MAC startup.
The assembly defines no 802.15.4 block runtime contract. An owner brings
the radio up in the order of ESP-IDF's `esp_ieee802154_enable`:

1. the common radio power and the IEEE 802.15.4 module clocks;
2. the shared PHY domain: registered, woken or joined as the
   [first PHY client](../../esp32s31/phy/README.md#first-and-last-phy-client);
3. the shared BTBB reference and the transmit-on delay;
4. the MAC reset and the masked foundation;
5. the interrupt owner;
6. the engine's `mac_init`, under the runtime's critical section;
7. the CPU route of modem interrupt source 132.

Stop reversed every step. A failure before shared state was touched rolled
back; a started PHY or clock transaction that failed was fail-stop. The
runtime was a process singleton: source 132 had one handler.

A consuming `detach` returns the MAC only after a completed stop and the return
of delivered RX slots, with no active operation, armed RX or pending timers.
Failure returns the complete owner. Software-only `disable`, `enable` and
`mac_init` cannot conceal an active operation epoch. The detached engine keeps
its unique storage and retained settings; `rebind` reuses that same storage with
no new claim or address construction. The S31 driver preserves its DMA placement
proof through this transition. Silence the CPU route before detaching and use
the existing IRQ deactivation/foundation return path before leaving PHY.

Operational ACK waits acknowledge timer0 while enabled. Successful ACK reception,
stopping an ACK wait and the hardware RX-ACK timeout stop timer0, sample and
clear its pending event while enabled, then disable that event. Starting a
new ACK wait enables timer0's event and clears old pending while the timer is
stopped, before programming and starting the next deadline. This prevents a
retained timeout from completing a later transmission with a false `NoAck`.
The S31 HAL consumes a saved affine event snapshot for runtime acknowledgement;
copyable observations grant no write authority. Before binding, `arm` consumes
both task and inactive IRQ owners, masks all events, installs the abort
baseline, issues STOP and both timer stops, then discards every event lane
without a sample through the existing IRQ part. All pre-bind pending is stale
and cannot be serviced. The fixed W1C image includes all fourteen lanes even
when masked status reads zero; its read preserves undeclared bits only.
The returned affine task owner has no operational methods. After the IRQ owner
moves into its handler, task activation with `Bound` publishes the handled set
without accessing the handler cell. Initial and later publication use the
same ten runtime events; timer0 is enabled only for an ACK wait. Polling instead
requires `Silent` after the same pre-bind discard. Inherited timer1 pending
cannot reach a new operation.
Masked ack is measured for timers0/1 and RX_ABORT reason26; masked visibility
and hidden pending on 7/10/13 remain unmeasured. No zero read establishes their
absence, and the startup discard does not prove producer drain.
Deactivation issues STOP and both timer-stop commands, samples, masks and
acknowledges its saved sample. These commands and selected acknowledgements do
not prove that every hardware producer drained.

Engine interrupt service rejects a sample containing an unnamed lane (7 or 13)
or the unsupported clock-match event (10) with `Fault::UnhandledEvents` before acknowledgement
or dispatch, preserving the complete event sample and entry state. The combined
MAC/engine owner consumes itself into absorbing `Quarantined` storage with no
recovery operations. The S31 PHY HIL image calls `fatal::reset_recording` with
those owners retained. Its new 12-byte versioned/checksummed record reports the
original fault at the next boot and survives console refusal.
SOURCE: `hw8-20261026-radio-local-05`, `ieee-timers.json`,
[PR #689](https://github.com/ermacv/open-esp-radio/pull/689): masking hides
timer0/timer1 status while retaining pending; a positive sample captured
before masking still clears it under mask. These facts do not establish
same-bit recurrence priority or the behavior of other event lanes.

## Enhanced ACK and transmit security

The engine calls its environment for each received 2015 frame that requests
an ACK and at each transmit SFD; no generator in this repository answers it.
A radio role answers as ESP-IDF's OpenThread port does (`esp_openthread_radio.c` at ESP-IDF
[`7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`](https://github.com/espressif/esp-idf/blob/7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe/components/openthread/src/port/esp_openthread_radio.c)),
and a generator keeps these rules:

- IE order (`ot_radio_enh_ack_generator`):
  1. the CSL IE template, when the receiver runs CSL (a nonzero period);
  2. the Link Metrics enhanced-ACK probing IE, when the frame's source is a
     probing initiator (`otLinkMetricsEnhAckGenData`);
  3. the configured header IEs, at most `ENHANCED_ACK_IE_CAPACITY` bytes.
- At the transmit SFD (`ot_radio_transmit_sfd_done`), with a nonzero CSL
  period, the CSL IE of the frame or enhanced ACK going out gets the period
  and the phase from the radio clock at that moment to the next sample time
  (`oer_ieee802154_mac::mac::csl`); with a zero period nothing is written. A
  transmitted frame's Time IE gets the time-sync sequence and the network
  time (the radio clock plus the network time offset,
  `oer_ieee802154_mac::mac::time_sync`); an enhanced ACK never carries one.
- A retransmission keeps its frame counter and key index, except that a CSL
  receiver takes a new frame counter on every retry: the CSL IE changes
  with each attempt.
- A secured ACK uses the keys of the interface the received frame matched
  (`mpf_index`; `enh_ack_set_security_addr_and_key`). The ACK takes the next
  frame counter of those keys and the frame's key index, which the port
  keeps as `s_ack_frame_counter` and `s_ack_key_id`. When the ACK must be
  secured, a frame that no single interface claims, or an interface without
  keys, gets no ACK.
- The generator also refuses a received frame it cannot parse and an ACK it
  cannot generate (`generate_enhanced_ack` fails) or secure. A refused ACK
  leaves the engine to deliver the frame unacknowledged.
