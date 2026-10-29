# AES service

`oer-services-aes` is the HIL AES vocabulary: `aes/key-unwrap` takes an
AES-128 KEK and up to 40 bytes of RFC 3394 wrapped key data and answers
`aes/key-unwrapped` with the plaintext or a typed refusal (length, integrity,
or the engine's own failure). `Engine` is the service's handler over any
`oer_crypto::KeyUnwrap` engine: the engine moves into each operation and
comes back with its response. The image and the host scenario share the
crate; `messages.lock` is checked by the crate's test (`descriptors`).

```console
cargo test -p oer-services-aes --features descriptors
```
