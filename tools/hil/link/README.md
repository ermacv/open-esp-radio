# HIL link

`oer-hil-link` is the host end of one board's serial line: it writes the
host's frames, decodes the device's, tracks the boot they belong to and
correlates each request with its response. It knows the protocol's `base`
module and nothing else; every other message is a header, a key and a payload
that the caller decodes as the type it expects (`Received::decode`). It
stores nothing.

| Item | Owns |
| --- | --- |
| `Link::start` | The worker thread that owns the serial line the stand opened |
| `Link::boot` | The running boot: its own Hello, or the answer to `base::GetHello` asked once at half the timeout |
| `Link::call` | One request and its typed response or `Rejected`; `Unconfirmed` without a response, `Rebooted` when another boot's frame arrives |
| `Link::take` | A kept message of the boot, such as an event of a request |
| `Health` | Decoder counters, device messages missing from a boot's sequence, host drops and boots seen |
| `Link::take_notes` | The device's `OER-DIAG` lines rendered as text; a garbled one stays visible as `OER-DIAG-ERROR` |

A frame of boot 0, another framing version or a repeated or reordered device
sequence fails the link. A late response to an unconfirmed request answers no
later request. Every wait ends on cancellation.

`test-support` provides a simulated device on a socket pair for other
packages' tests.

```console
cargo test -p oer-hil-link
```
