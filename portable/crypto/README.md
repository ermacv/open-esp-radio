# oer-crypto

Owned cryptographic operation contracts and the mode algorithms behind them,
shared by every protocol that needs them.

One operation is one request. `KeyUnwrapRequest<N>` owns an AES-128
key-encryption key and a buffer of capacity `N` and moves by value into a
`KeyUnwrap<N>` engine. The engine moves itself into the operation and comes back
with `KeyUnwrapResponse<N>`, which returns the same buffer: the plaintext on
success, cleared on refusal. A caller holds at most one operation per engine,
so there is no busy refusal. Consuming the request erases the key; `Aes128Key`
and `KeyData` erase themselves on drop. Errors are properties of the wrapped
data (`InvalidLength`, `IntegrityCheckFailed`). An engine whose hardware may
still reach a request's storage keeps that storage until it confirmed the stop,
even when its operation is dropped.

The RFC 3394 algorithm has one copy, in `rfc3394`: the resumable
`Rfc3394Unwrap` machine performs one block per `step`, so an engine may yield
between blocks, and `wrap` wraps to completion. Both run over the synchronous
`block::BlockCipher`; `block::Aes128Blocks` loads a key for a run of blocks and
erases it when the keyed view drops. `block::BlockKeyUnwrap` is the engine of a
synchronous primitive: its operation is ready on its first poll. Only the
machine builds a response, so every engine's plaintext passed the same
integrity check.

With `test-support`, `conformance::key_unwrap_rfc3394` runs an engine through
the RFC 3394 §4.1 vector, a changed ciphertext, two invalid lengths and a
request after a refusal, driving a pending operation through a caller-supplied
step.

The crate is sans-IO: no unsafe code, allocation or runtime panic path. It
denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and
disallowed-macro Clippy lints, and its local Clippy configuration includes the
common bans and the complete sans-IO catalogue.
