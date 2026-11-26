# oer-portable-aes-software

The RustCrypto AES-128 cipher as a synchronous AES-128 engine of
`oer-portable-crypto`: `SoftwareAes128`, whose keyed view erases its key
schedule on drop. Its `Stop` is `core::convert::Infallible`: it cannot stop,
so its callers have no engine error path. A composition without an
accelerator, or a protocol's own software step, chooses it as a static type;
no code answers an accelerator's stop with it.

The external `aes` crate is a normal dependency here alone among the
product crates (the layers' external-crate rule).

```console
cargo test -p oer-portable-aes-software
```
