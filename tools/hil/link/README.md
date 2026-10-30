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
| `Link::call` | One request and its typed response or `Rejected`, in a slot reserved before the request leaves; `NotSent` when it did not leave the host, `Unconfirmed` without a response, `Rebooted` when another boot's frame arrives |
| `Link::cursor`, `Link::take` | The running boot's other messages from a point on, such as a request's related events, also those before its response |
| `Link::completeness` | Whether a cursor's interval lost a message (`Lossy`) or its boot ended (`Ended`) |
| `Health` | Decoder counters, boots seen, the running boot's and the last ended boots' missing, displaced and unread messages, lost diagnostic lines |
| `Link::take_notes` | The device's `OER-DIAG` lines rendered as text and how many were lost; a garbled one stays visible as `OER-DIAG-ERROR` |

Every queue is bounded. Host frames wait in `OUTBOUND_CAPACITY` places and
at most `MAX_PENDING` requests wait for responses; either limit refuses a
request before it leaves (`NotSent`), so sending it again is safe. The
running boot keeps `EVENT_CAPACITY` messages: a newer one displaces the
oldest, which is lost for every cursor whose interval holds it, as is a
message the device's sequence skipped. A new boot starts with nothing kept;
the ended boot's counters stay in `Health` for `PAST_BOOTS` boots.
Diagnostic lines keep `NOTES_CAPACITY` lines the same way.

A frame of boot 0, another framing version or a repeated or reordered device
sequence fails the link. A late response to an unconfirmed request answers no
later request. Every wait ends on cancellation.

`test-support` provides a simulated device on a socket pair for other
packages' tests.

```console
cargo test -p oer-hil-link
```
