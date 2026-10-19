# Hardware calibration cross-check

Compares the cold PHY calibration of the pinned vendor firmware with the
production calibration on the same ESP32-S31 board. The comparison uses the
reviewed `phy_param` relation that the Blobray tracking scenario also uses
([`oer-esp32s31-phy-relation`](../relation/README.md)'s `committed`).
The production side goes through the same projection functions as the
comparison probes (its `projection`).
It does not depend on Blobray peripheral models.

This package is a library: what is compared and how. No HIL scenario runs
the capture now: the `phy` family's `vendor-calibration` workload and its
scenarios were deleted with the production images they booted (), and
return with the HIL architecture of ; the tag `legacy-wifi-2026-10-16`
keeps them. The host comparison port is
[`oer-hil-family-phy`](../../../host/family/phy/README.md). The capture below
describes what the removed workload did and what a new one must reproduce.

## Capture

Cold calibration measures TX and RX DC and IQ that another transmission
would bias, so the scenarios claim the 2.4 GHz band. For each of the
table's `boots` rounds the workload takes these steps:

1. Flash the [vendor calibration firmware](../../../../verification/esp32s31/hil-vendor/README.md),
   reset the board and keep the boot's `phy_param` report. Then request
   every readable register of the published radio-PHY ownership partition
   (`RadioPhyPeripherals` of `registers/esp32s31/policy/api.toml`,
   registers from the published SVD). A read that resets the chip, such as
   a register whose clock domain the Wi-Fi calibration leaves off, is
   recorded as unreadable and the reads continue after the new boot. Then
   request every register of the analog image: each analog-I2C register a
   reviewed `PhyI2cField` of the PAC API policy occupies (its
   `register-image`), read through the ESP-IDF analog-I2C driver.
2. Write the scenario's production image back and boot it with a fresh
   startup artifact path, so the boot calibrates fully and publishes its
   retained calibration. Then read the radio-PHY register image at the
   indices the vendor boot of the round read, and the whole analog image
   (one production analog-I2C read per register). The former HIL messages
   `PhyRegisterImage` and `PhyAnalogImage` (tag `legacy-wifi-2026-10-16`) carried these reads;
   no current image serves them.

`lifecycle = "ieee802154"` and `"ieee802154-restart"` read the registers
with the IEEE 802.15.4 radio instead of Wi-Fi holding the PHY. The vendor
side is the [IEEE 802.15.4 reference firmware](../../../../verification/esp32s31/hil-vendor/ieee802154-reference)
configured on channel 15 at 21 dBm and receiving, after one driver disable
and enable for the restart point, read through its `PEEK` and `ANALOG`
commands; a read that resets or hangs the chip is recorded as unreadable
after a board reset. Production ran the former `diagnostic-ieee802154-radio`
image's IEEE 802.15.4 session on the same channel. These points compare
register state only.

At these points the vendor side can also investigate what production does
not publish. Each `vendor_windows` entry (`{ address, words }`) is read with
`PEEK` into the boot's `windows`; `vendor_transmit` then sends one data
frame without CCA or acknowledgement request and reads the analog image and
the windows again (`transmitted`). `vendor_only` boots only the vendor
firmware and compares nothing: production serves only its register
partition, so the windows are evidence for an investigation, not a
comparison.

`lifecycle` selects when both sides report their registers. `cold` reads
them after the cold calibration and the Wi-Fi bring-up. `restart` first
restarts the Wi-Fi radio once: the vendor firmware stops and starts its
Wi-Fi client, and requires that stopping it released every PHY modem, which
closes RF; production runs its idle radio restart (`RestartRadio`) and
requires that RF was closed and woken. A vendor read that resets the chip is
followed by another restart before the reads continue. `image` and
`vendor_project` replace the point's own production image class and vendor
project.

Alternating the sides exposes both to the same board temperature drift.

## Compare

`compare::compare` does the following:

- Decodes each production artifact with the target's own codec.
- Rebuilds the retained PHY state and projects it onto the relation's output
  words.
- Reads the same fields from each vendor `phy_param`.

Cold calibrations vary from boot to boot, so the vendor's own spread is the
reference. A field's margin is the widest range its vendor boots span in any
one element. An element passes when every production value lies within its
vendor range widened by that margin.

Every field carries a review in [`tolerances.toml`](src/tolerances.toml), which
the library compiles in, with these parts:

- whether its elements are signed;
- which bits the relation models (`mask`);
- whether it describes the environment rather than calibration (`excluded`);
- the one lifecycle point it applies to (`lifecycle`), when the difference
  comes from the point rather than the state; a name may carry one review
  per point as an array of tables, and two reviews applying at one point
  fail;
- the reason.

A field without a review leaves the verdict INCOMPLETE.

The summary records:

- the capture date, lifecycle point and verdict;
- both images;
- each field's margin and its vendor and production ranges;
- the excluded fields with their reasons;
- the vendor byte ranges no compared field covers;
- the register image: registers compared and matched, the registers
  outside their vendor range widened by their own vendor spread with both
  ranges, reviewed exclusions, registers whose read reset the chip, and
  vendor-readable registers production did not report;
- the analog image, under the same rule and reviews.

## Limitations

Both sides are read at the same lifecycle point: the vendor firmware starts
its Wi-Fi client without a connection, with power save off and promiscuous
RX on its home channel, since production's role-neutral Wi-Fi bring-up
enables RX on its initial channel and has no modem sleep. Production's missing modem sleep is a behavior difference
outside this calibration comparison.

The relation covers these parts of the committed calibration, in the order
the tracking roots commit them:

- references, channel and bandwidth;
- TX DC rows;
- RX DC banks;
- DCODE codes, status bytes, RX-gain table last indices and sensor index.

Committed state outside `phy_param`, and `phy_param` bytes outside the
relation, are reported as uncovered, not compared.

```console
cargo test -p oer-vendor-phy-esp32s31
```
