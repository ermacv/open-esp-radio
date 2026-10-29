# Radio-free system workloads

`aes-key-unwrap` serves individual host requests through the same
`compose::crypto::wifi_key_unwrap` selection and public `oer_crypto::KeyUnwrap`
API as the Wi-Fi facade. Its exclusive HIL image starts no station. Images
without its feature neither mount nor advertise this service.

The host checks RFC 3394 §4.1 (the §4 vector with the product's AES-128 KEK)
and generates 768 more requests with 32 changing KEKs and plaintext lengths
16, 24, 32, 40, 64, 120, 248 and 504 bytes. Every length runs a valid unwrap,
an integrity refusal and another valid unwrap. A software engine checks every
answer; the board contains no software cipher or autonomous vector loop.
Replies retain operation identity, cumulative completed/succeeded/refused
counts, plaintext or a typed error. The first mismatch records the complete
request, software result and board reply in `observations.json`; inputs are
public test material. The HIL protocol records every board reply.

Run with `cargo hil run aes-key-unwrap --board s31-a --owner OWNER`.
This investigation isolates the product engine; it makes no Wi-Fi claim.
