# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: the request intake (`base`), console serialization and
the reset-retained post-mortem record. The
[esp32s31 HIL agent](../../../firmware/hil/esp32s31/README.md) composes these modules with
its executor and console. Host tests exercise the same compiled code the
firmware links.

| Feature | Modules |
| --- | --- |
| none | `base`, `console`, `postmortem` |
| `health` | `health` diagnostic storage |

Terminal completion waits are bounded to sixteen receivers. Exhaustion returns
`ReceiverLimit` with the requested sequence, last published position and capacity.
The image retains this impossible-state fault through its existing fatal record,
keeps the operation's owners quarantined and resets. It never performs the USB,
watchdog, panic-injection or radio-release action after a failed barrier.

`Console::run` returns a `RunFault` owning the endpoint on an invalid read length
or counter-borrow conflict. `link_health` reports counter-borrow conflicts as a
`ConsoleFault`. Checkpoint, rate and health category count overflows return typed
faults before mutation; images retain their observations and reset.

The retained record is a value codec; placement and reset belong to the image.
Const size, alignment and offset checks fix its layout. `AnyBitPattern` proves
validity of arbitrary retained bytes; checkpoint storage uses primitive native
words with explicit byte-preserving conversions instead of a `Pod` byte-cast API.
No retained capacity or allocation changes.

Run the tests:

```console
cargo test -p oer-hil-agent
```
