# AES scenario

`cargo hil run aes` checks the product hardware RFC 3394 key unwrap
(`oer-crypto` over the S31 AES engine) on the `esp32s31-rpc` image through
the [AES service](../services/aes/README.md).

| Case | Checks |
| --- | --- |
| `rfc3394-known-answer` | RFC 3394 §4.1 unwraps to its key data |
| `reference-wraps` | Nine reference cases, 128-, 192- and 256-bit key data under three KEKs from a fixed sequence, wrapped on the host, unwrap to their key data |
| `integrity-refused` | One flipped bit at the first, a middle and the last position is refused as `IntegrityCheckFailed` |
| `length-refused` | 0, 8, 16, 20 and 39 bytes are refused as `InvalidLength` |

The host reference wraps with RustCrypto's AES-128 by RFC 3394 §2.2.1 and
shares no code with the image; its test reproduces the RFC vector.

```console
cargo test -p oer-hil-scenarios-aes
```
