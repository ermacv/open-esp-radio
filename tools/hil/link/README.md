# HIL link

The host/target exchange, console capture and protocol correlation for HIL runs.

## Instrumentation

`SerialCapture::read_instrumentation` pins the boot, capture identity, epoch
and expected schema key, then drains generic record and counter pages.
The pure `instrumentation::Pages` assembler rejects changed identities,
counts, repeated/skipped pages, incomplete responses and duplicate counter
ids before accepting a page. A completed drain retains device completeness
failures; it cannot turn lost history or saturated counts into exact evidence.
Domain tools decode records only after checking their dictionary schema.
