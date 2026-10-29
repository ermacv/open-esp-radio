# Lower-MAC conformance image

`diagnostic-lower-mac` exercises the owned ESP32-S31 lower-MAC port and
compares returned owners and outcomes with the host reference model.
The image selects an `oer-trace::Observer` that forwards small driver values
to a fixed bank of six atomic event-kind counters. Product owners select
`Noop`; observation points disappear without `oer_trace`.

Publication, terminal, EDCA table/record/draw and rollback counts are cleared
at each case's measurement boundary. Counts contain no publication order,
frame bytes or retained history. Actual identities, ownership and outcomes
come through typed port terminals. The capture description and counter-page
endpoints read live atomics with capture identity 1 and epoch 0; the HIL boot
envelope identifies the boot. A client coordinates case boundaries when it
compares counter pages. There is no recorder, frame snapshot or trace clock.

The image declares `trace` and `debug-control`. Test holds and injections under
`oer_debug_control` force the required paths independently of observation.
Retained fault and postmortem records have their separate failure lifecycle.
