# Radio health snapshot — first-commit sketch for #383

This is the protocol/field sketch requested before implementation in #383.
No message, firmware behavior, allocation or register access is added by this
commit. The names below describe the proposed contract, not served endpoints.

## Capture and ownership

One host report joins a counter window, signal/temperature observations,
existing tracking decisions and intermediate measurements of the existing
DC/IQ/PWDET operations. The experiment fixes peer, channel, rate and configured
power and records source and vendor identities. Configured power and detector
codes are not claims of measured antenna power.

The proposed server is a separate `diagnostic-radio-health` feature of the
existing S31 diagnostic PHY package. Existing PHY capture, natural tracking and
RX-window operations retain RF, clock and engine ownership. Read/measurement
hooks observe those operations; they do not choose CCA/AGC/NF policy, start a
second calibration, reset statistics or release an RF owner between start and
capture. Production observers remain disabled and stateless.

The host assembles the report from typed messages without linking HAL/PAC.
Protocol fields contain identities and units, not MMIO addresses, register
layouts or arbitrary register-write commands.

## Proposed message set

The new optional `health` module uses new paths and schema-derived keys in
`messages.lock`; existing keys and the framing version remain unchanged.

| Endpoint | Response and meaning |
| --- | --- |
| `health/begin` | `HealthStarted`: observation id, capacity and native boot identity; arm collection before existing calibration/capture operations |
| `health/window` | `HealthWindow`: run one bounded existing RX/natural-tracking window; report actual epoch, channel and sampling brackets |
| `health/finish` | `HealthFinished`: freeze record counts, validity, overflow/failure and cleanup evidence after started operations finish |
| `health/counters/read` | `CounterPage`: immutable before/after counter records |
| `health/signals/read` | `SignalPage`: immutable NF/RSSI/temperature points |
| `health/tracking/read` | `TrackingPage`: existing decisions and observed intervals |
| `health/measurements/read` | `MeasurementPage`: existing DC/IQ/SAR/PWDET results |
| `health/release` | `HealthReleased`: retire the observation id after host collection |

Every page request carries observation id, epoch, first index and count.
Reads reject a stale id/epoch, an unfinished observation or an invalid range.
Counts cannot change after finish. The initial page limits are 16 counters,
8 signal points and 2 complex tracking/measurement records; their worst-case
postcard encoding must fit the existing 560-byte payload limit. That limit is
not enlarged. Unsupported values and unknown provenance are explicit variants
or `None`, never plausible zero samples.

## Fields and interpretation

| Record | Fields and constraints |
| --- | --- |
| Identity/window | boot identity, observation id, capture/MAC epoch, channel, radio, configured peer/rate/power, observed rate where available; timestamps use one device monotonic clock per boot |
| Counter cut | epoch/reset continuity, first/last read timestamps, profile, counter kind, raw value, width and semantics; the multi-register cut is best effort, not atomic |
| Counter pair | before/after cuts, modulo delta only for event counters in the same epoch; reset/epoch changes invalidate the delta; multiple wraps cannot be inferred |
| Signal point | timestamp, channel/gain/rate where observed, separate NF raw representation ids and values, native decoded NF/RSSI with declared units, and associated temperature provenance |
| Temperature | raw/native value and scale/profile, acquired/retained/read-only provenance, acquisition/read interval when known, optional age and reference values; an inherited sample has unknown age until an actual acquisition is observed |
| Tracking | native operation/branch, due/not-due/unknown, ran/skipped/failed, committed outcome, timestamps and invalid/incomplete state; consume existing inspection/observation results rather than running another planner |
| Deafness | observed baseband/RX-off interval edges and their source; request/grant duration is not proof of silence; overlapping parent/child intervals are not summed |
| DC/IQ measurement | request/iteration, channel/gain/configuration, I/Q/power sums, start/ready/capture times, completion/failure, correction and residual if already measured by the operation |
| SAR/PWDET measurement | request/measurement/sample identity, channel/gain/tone/configuration, SAR/detector result, readiness/capture times, completion/failure and existing final correction/result |

The S31 adapter reuses the typed RX primary counters (16-bit wrap), decoder
counters (10-bit wrap) and TX words (32-bit wrap). The signed CFO accumulator
sample is not an event counter and is not labelled Hz. Native TX labels `track`
and `trcts` retain their limited known meaning. A fresh residual is not acquired
for reporting: absence of an existing post-correction measurement is explicit.

The reference side uses its existing readable-register/analog interface and
keeps its firmware SHA in the report. Vendor fields without equivalent native
instrumentation remain explicitly unavailable; they are not reconstructed from
our trace. Golden R1 files are not overwritten.

## Proposed diagnostic storage decision

The proposed retained health storage has a **total ceiling of 128 KiB PSRAM**,
including its metadata, and at most 1024 native records. Actual capacity is
derived from the chip-compiled record layout and reported by `HealthStarted`.
This is a new diagnostic-only allocation and needs the owner's budget decision
before implementation; no such allocation exists in this commit.

The producer copies bounded native values during the existing operations.
Encoding/UART happens after operation return, outside the sensitive RF poll.
There is no transport backpressure inside calibration. Overflow preserves an
explicit count/status and makes the observation INCOMPLETE. It cannot pass as a
complete snapshot. Started PHY futures are driven through their existing
completion/cleanup contract.

## Validation and stops

Tests cover wire round trips and the current frame limit, epoch/range handling,
modulo widths, unknown/reused sample age, invalid/overflow traces, host page
continuity and cleanup evidence. Time comes from the existing virtual clock.
The host decoder lives in a hardware-free crate.

Production w0–w3 are compared byte-for-byte or against the owner's thresholds;
the new diagnostic image reports its actual memory/frame footprint. Tests,
clippy, formatting and chip CI precede one controlled vendor/source example,
not a run series. Text evidence goes to `~/dev/oer-notes/evidence/383/` and
binary artifacts to `~/.local/share/open-esp-radio/evidence/383/`, with SHA-256
in notes.

A destructive counter read or conflict with an active engine is a stop for an
owner decision; in particular there is no parallel drain of the 802.15.4 engine's
counters. A threshold failure after the permitted pass is reported and stopped.
