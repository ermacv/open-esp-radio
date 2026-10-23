# Radio health snapshot

`diagnostic-radio-health` is an ESP32-S31 diagnostic variant of the PHY image.
It collects existing calibration completions, a finite Wi-Fi RX window and one
natural tracking evaluation. It selects no new CCA, AGC, noise-floor or tracking
policy. The PHY/RF owner remains borrowed until each started operation returns.

## Exchange

The optional `health` protocol family has independent paths and schema keys.
The existing framing version and 560-byte postcard payload limit apply.

| Request | Response |
| --- | --- |
| `health/begin` | `health/started`: boot/id, native capacity and storage size |
| `health/window` | `health/window-finished`: RX cleanup and natural tracking result |
| `health/finish` | `health/finished`: immutable counts, dropped records, failure and RF cleanup |
| `health/counters/read` | `health/counters`: at most 16 records |
| `health/signals/read` | `health/signals`: at most 8 records |
| `health/tracking/read` | `health/tracking`: at most 2 records |
| `health/measurements/read` | `health/measurements`: at most 2 records |
| `health/release` | `health/released`: retire the observation |

The host arms before an explicit full `phy/calibration/start`. That existing cold registration
binds the observation to its nonzero MAC capture epoch. A failed registration
can be frozen with epoch zero; that report cannot be complete. Collection is
one cold observation per boot. Release retires the id, without making the
registered PHY a new cold boot.

Begin records a fixed peer, channel, configured peer rate in kbps and configured
peer power in dBm. These configuration values do not certify received rate or
measured antenna power. Window runs the existing finite promiscuous RX
transaction, stops/reclaims its ring, then evaluates natural tracking through
its ordinary exclusive path. Finish closes RF without running RX a second time.
Pages require a matching observation/epoch and a frozen report. They reject
empty, oversized, stale and out-of-range requests. Counts never change after
finish. Encoding and UART paging happen after operation return, outside PHY
measurement polling; collection has no transport backpressure.

## Interpretation

Counter cuts are best-effort multi-register observations bracketed by device
monotonic microseconds. They do not clear statistics. Primary RX counters wrap
at 16 bits, decoder counters at 10 bits and the four TX words at 32 bits. Delta
is modulo only within an unchanged epoch; the number of full wraps cannot be
inferred. The CFO sample is a signed accumulator scaled by 40, not an event
counter or a frequency in Hz. `track` and `trcts` retain their vendor labels.
No IEEE 802.15.4 engine counter is drained.

NF records preserve the twelve-bit sixteenths-dB code and native quarter-dB
conversion from the same read. The separate ten-bit NF/ambient-RSSI bank has
unknown read semantics and is not read by this image. RSSI and raw PHY family /
rate code come from a normalized RX descriptor of the configured transmitter.
The configured capture channel is not evidence of per-frame gain or channel.

Temperature uses native sensor units, with the existing acquisition provenance:
unobserved, undated, overlong, clock-reversed or an acquisition window. Reading
retained state does not renew its acquisition timestamp. Age is unknown without
a valid acquisition window; there is no Celsius conversion.

Tracking inspection copies the existing retained predicates and references;
it does not execute a second planner. Actual branches report started / ran /
skipped / failed and committed outcome bits separately. Only baseband-off
edges describe observed deafness. Grant-protect intervals are requests, not
proof of RF silence. Parent/child intervals overlap and must not be summed.

RX DC/IQ records copy existing ready/read/disable atom results, raw I/Q/power
sums, request/measurement/gain/stage context and existing accepted coefficients.
Their timestamps bracket software atoms, not exact hardware-ready edges.
SAR/PWDET records preserve existing tone, trigger, readiness, timeout and sample
completions. Retained detector parameters are identified as retained data.
Unknown channel/gain and absent post-correction residuals remain explicit;
collection never starts another measurement to fill them.

The diagnostic store retains at most 1024 native records and checks its entire
storage, including metadata, against 128 KiB. Overflow is counted and makes the
report INCOMPLETE. Failure and cleanup evidence remain readable after finish.
The hardware-free host assembler checks identity, page continuity, counts,
widths and counter brackets. Its paired example retains vendor/source hashes
and distinct timebases; unavailable vendor instrumentation is explicit. It
never overwrites golden calibration references.
