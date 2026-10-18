# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), console serialization and
the reset-retained post-mortem record. The
[esp32s31 HIL agent](../targets/esp32s31/README.md) and the
[esp32c5 HIL agent](../targets/esp32c5/README.md) compose these modules with
their executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `console`, `postmortem` |

Run the tests:

```console
cargo test -p oer-hil-agent
```
