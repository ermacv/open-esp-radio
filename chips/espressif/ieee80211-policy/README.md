# Espressif IEEE 802.11 transmit policy

`oer-espressif-ieee80211-policy` is the Espressif family's policy data for
the portable transmit algorithms above the
[lower-MAC port](../../../protocols/ieee80211/lower-mac/README.md). The algorithms are
written once in [`oer-ieee80211-upper-mac`](../../../protocols/ieee80211/upper-mac/README.md)
and `oer-ieee80211-mac`; they take limits, rate ladders and estimates as
parameters, and this package supplies the values the vendor stack uses. It
is a `family` package (`family = "espressif"`): chips of the family may
depend on it, and portable code never does.

| Module | Contents | Source |
| --- | --- | --- |
| `rate_schedule` | The nine rate-schedule arenas in their post-`rcAttach` state, the `rcGetRate` record walk, the `rcReachRetryLimit` publication budget and the `rcUpdatePhyMode` rate-to-record maps | `libpp.a[trc.o]` |
| `rate_control` | The per-association rate control over those schedules: schedule selection at association (`StaRateControlAssociation`, `rcUpdatePhyMode`), the ordinary schedule lowered under retry pressure (`rcTxUpdatePer`), the independent A-MPDU schedule moved by BlockAck windows (`rcUpdateTxDoneAmpdu2`), the ACK-SNR filter and the beamforming report rate; values only; an aggregate's BlockAck evidence enters through `BlockAckSnr`, and one without an SNR sample leaves the ACK-SNR filter as it is, a reviewed difference from the vendor | `libpp.a[trc.o]`, `libpp.a[if_hwctrl.o]::ic_set_trc`, `libnet80211.a[wl_cnx.o]::ic_set_sta` |
| `rate_code` | `wifi_phy_rate_t` codes of the records as portable `PhyRate`s, by schedule | `esp_wifi_types_generic.h` |
| `retry_ladder` | The ordinary-MPDU retry ladder of legacy, HT and HE rates (`EspressifRetryLadder`, a portable `RateLadder`) | `libpp.a[trc.o]::{rcGetRate, rcUpdatePhyMode}` |
| `rate_selection` | The rates of every frame a station sends (`EspressifRateSelection`, a portable `RateSelection`): a probe once at 1 Mb/s, other non-data frames on the default control record until connected and on the association's after, data from the rate control down the retry ladder within its record's budget; builds only for a port without 5 GHz whose receive metadata measures the signal (`SignalMetadata`) and whose ACK evidence carries the SNR (`AckSnr`) | `libpp.a[trc.o]::{rcGetSched, rcGetRate, rcReachRetryLimit}`, the former S31 station path |
| `lmac` | Short and long retry limits (32), ACK-timeout accounting, the RTS threshold (2346 octets), the A-MPDU MSDU lifetime and aging margin, the A-MPDU retry policy and the default contention per access category | `libpp.a[lmac.o]::{lmacInit, lmacInitAc, lmacProcessAckTimeout, lmacMSDUAged}`, `libpp.a[pp.o]::ppResortTxAMPDU` |
| `he_txop` | The HE TXOP duration RTS byte budget (`EspressifHeTxopRtsBudget`, a portable `HeTxopRtsBudget`) and the peer packet-padding code | `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` |
| `ccmp` | The CCMP transmit packet-number step of three | `libnet80211.a[ieee80211_crypto_ccmp.o]::ccmp_encap` |
| `connection_coex` | The coexistence events (45, 46) and priorities a reconnecting station's Probe Request, Authentication, Association and EAPOL frames request, and the `ConnectionFrameCoex` port a chip implements | `libpp.a[pp.o]::pp_coex_tx_request`, `libpp.a[pm_coex.o]::pm_coex_reconnect_policy` |

The values were recovered from the ESP32-S31 libraries pinned in
`chips/esp32s31/vendor/artifacts.toml`; each item's `SOURCE` comment names
the vendor function it was read from. A chip of the family whose pinned
libraries differ needs its own reviewed values before it uses them.

The schedule walk, the retry ladder and the adaptive rate control (`trc`),
which chooses a frame's first rate, live here, not in the portable package,
because they are inseparable from the vendor schedule bytes. The rate
control holds schedules, not PHY rates: a chip turns a schedule into its own
rate (the ESP32-S31 MAC's `StaTxRatePolicy`, with its certification
overrides) and programs its own beamforming report-rate registers, and a
portable station uses it through `EspressifRateControl`, the station's
`StaRateControl` seam, which decodes its schedules through `rate_code`.

The portable `RatePeer` supplies all negotiated legacy rates, including the
extended element. A DSSS/CCK-only association selects the 11b schedule.
Every legacy rate from control, data, retry and fallback is constrained to
that set: retain an admitted rate, otherwise choose the fastest admitted rate
no faster than it, or the slowest member if all are faster. This preserves
the publication budget and never adds OFDM to a B-only peer. HT/HE retain the
same constraint when their retry ladder reaches legacy rates.

Rate-control schedule literals are bounds-checked at compile time with an
array-length equality. Initial PHY-mode selection chooses those complete
records directly; it has no out-of-table index or clamp to the last record.
The HT metric comparison uses the unadjusted threshold before selecting its
HE record two positions lower. Unknown peer rate encodings retain the
vendor's record-zero result. HT fallback uses the MAC's checked
`HtMcs::SINGLE_STREAM_MAX` value; neither path can panic while processing a
transmission.

`RateScheduleRef` retains a reference to the checked twelve-byte record, with
private fields. Its kind and record cannot be relabelled independently; the
record's attached index is read through `index()`. The arenas are arrays of
records and consumers destructure fixed record arrays. On the 32-bit S31 a
handle occupies eight bytes, where a kind/index pair would take two; the
table bytes and publication budgets do not depend on it.

The rate-code maps return `Option<u8>`: an unknown code, a subtraction
underflow or a position outside the family's arena has no index. Valid
callback mappings retain their recovered values. The largest private arena
has 16 records; adding four byte-sized retry counts uses `u16`, so neither
its attached index nor the retry sum wraps.

PHY decoding first bounds each MCS code to its HT or HE wire domain;
subtracting the domain base therefore yields exactly 0–7 or 0–9. Likewise,
the retry ladder admits only MCS 0–7 or 0–9 before reversing their record
indices to 1–8 or 1–10. These subtractions cannot wrap or saturate, and an
unsupported starting rate cannot alias the highest-rate record.

Retry selection either represents the complete record walk or refuses the
plan when it needs more than four stages. Channel filtering preserves every
accepted stage and its transmission count within that original capacity.

The HE TXOP estimate keeps the vendor's single-precision operation order and
16-bit table result. Its checked threshold and rate bound the intermediate
integer arithmetic inside `i32`; the final halfword conversion is intentional
modulo arithmetic. When positive remaining time fits no complete symbol,
subtracting the SERVICE/tail bits produces a signed result whose stored
halfword is 65533. It must not be clamped to zero.

Rate observations carry a nonzero MPDU count and an explicit ACK outcome, or
a `BlockAckTally` whose acknowledgements cannot exceed its attempted MPDUs.
Those proofs survive through the vendor window's division; observing a
checked tally has no error path after mutation. An association without an
A-MPDU controller returns `AmpduUnavailable` directly before changing its
rate or ACK-SNR state. There is no separate refusal counter or pending error
slot for a caller to poll.
The crate uses the canonical strict lint block.

## Known defects

- **`rate_control.rs::ScheduleSelection::Invalid` keeps the current
  schedule:** `update_tx_per` installs a schedule only on `Selected`, so an
  invalid selection leaves a possibly too-fast rate in use.
