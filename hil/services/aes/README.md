# AES service

`oer-hil-services-aes` is the HIL AES vocabulary: `aes/key-unwrap` takes an
AES-128 KEK and up to 40 bytes of RFC 3394 wrapped key data and answers
`aes/key-unwrapped` with the plaintext or a typed refusal (length, integrity,
or the engine's own failure). `Engine` is the service's handler over any
synchronous AES-128 engine (`oer_portable_crypto::block::Aes128Engine`): each request
runs to completion in the handler's call (`oer_portable_crypto::block::unwrap_now`),
and the engine is free again on return. The image and the host scenario share
the crate.

```console
cargo test -p oer-hil-services-aes
```
