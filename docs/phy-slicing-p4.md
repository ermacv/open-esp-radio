# PHY slicing P4: common calibration

Tracking issue: <https://github.com/ermacv/open-esp-radio/issues/328>.
Baseline: `1234079785886c41a23cbe4d84f74c1737d11837` (`origin/main`).

This work moves the common calibration bracket onto the existing P1 step
engine: DCODE, RX gain DC and publication, DC/IQ acquisition, and channel
restoration. Cold registration and compiled probes must share the affected
measurement implementation. RF and bus ownership remain held across every
checkpoint; parent state commits only after the accepted child completion.

The acceptance limits relative to the baseline are:

- Executable growth at most 4096 bytes per w0, w1, w2 and diagnostic image.
- Enclosing future growth at most 384 bytes; new or growing frames and computed
  stack prefixes at most 64 bytes per root.
- No new writable RAM sections, larger allocated stacks, static scratch,
  parent union, unsafe code or uninitialized storage.
- Every engine poll at most 50 microseconds in the hold measurements.
- Existing T1, T3 and T7 limits and the 116-byte power future limit remain.

Qualification requires host semantics and cancellation tests, reverse dependency
checks, formatting and clippy, four ordinary RV32 builds and audits, all affected
probes and compiled suites, both 3-by-5 hold campaigns, and the four R1 comparisons
against the frozen Wi-Fi N40/N60 and IEEE 802.15.4 N45/N50 references. Executable
accounting includes both sized symbols and the remaining executable gaps.

Implementation and qualification are pending. A draft PR does not establish any
of these acceptance results.
