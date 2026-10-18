# HIL wire contract

Host and firmware must use the same framing version, defined by
[`FRAMING_VERSION`](src/framing.rs), and must agree on the key of every
message they exchange. A frame of another framing version is rejected before
its header is interpreted; a key the host does not know fails the capture,
since nothing could decode or check that message.

## Framing

Source types are authoritative. A frame is:

```text
00 00 | COBS(fixed-header | postcard(body) | CRC32C) | 00
```

The 40-byte little-endian header contains magic `ORHL`, framing version,
request/device kind, boot ID, message sequence, session ID, request ID, the
message's key and the payload length. CRC covers header and body. A wrong kind
or framing version is rejected before the body is decoded; the key selects the
body's type. Host and target treat decode errors, sequence gaps and
bounded-queue loss as protocol failure.

The optional `async-io` feature exposes `write_frame` for an already encoded
frame. It writes all bytes and flushes before returning success. Callers must
bound the complete operation, including flush: accepting bytes alone does not
complete a USB transfer whose final packet is full.

## Image keys

A boot's first frame is its `base::Hello`: how many keys the image serves, a
digest of them and its payload limits. The host pages the sorted keys with
`base::GetImageKeys` and checks them against the digest. An image serves
the base module's endpoints and the endpoints of its own request set; any
other key is refused as unsupported.

## Discovery and boot identity

Read-only attachment discovers a running runtime with `base::GetHello` in an
envelope whose boot ID and session ID are zero. The reply is a correlated
`Hello` carrying the current nonzero boot ID; all subsequent requests bind to
that boot. This exception admits no other request. Firmware that rejects
discovery must be explicitly updated; the host never resets it to obtain
status. An attachment begins at the current
event sequence; a reset-driven qualification capture still requires the boot
`Hello` at sequence zero. Both modes reject later sequence gaps and reboot.

`network::GetStatus` and `base::GetLinkHealth` do not initialize the runtime
or consume retained results. A status observation preserves cumulative link
counters
instead of applying a new workload's acceptance criteria to previous activity.
