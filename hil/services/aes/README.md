# AES service

`oer-hil-services-aes` is the HIL AES vocabulary: `aes/key-unwrap` takes an
AES-128 KEK and up to 40 bytes of RFC 3394 wrapped key data and answers
`aes/key-unwrapped` with the plaintext or a typed refusal (length, integrity,
or the engine's stop). `Engine` is the service's handler over a synchronous
AES-128 engine that may stop (`oer_portable_crypto::block::Aes128Engine` with
`Stop = EngineStopped`) and a reader of its driver's record: each request runs
to completion in the handler's call (`oer_portable_crypto::block::unwrap_now`),
a stop is answered with what the record holds of it (`EngineStopped { last }`),
and the engine is free again on return. The image and the host scenario share
the crate.

```console
cargo test -p oer-hil-services-aes
```
