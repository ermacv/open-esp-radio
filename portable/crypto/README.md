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

The RFC 3394 algorithm has one copy, in `rfc3394`. `rfc3394::unwrap` runs the
index-based unwrap to completion in place in the request's buffer, over a
synchronous block engine; nothing moves the buffer between blocks. A length
refusal loads no key. The integrity register and the block are erased on
every exit; any refusal clears the response buffer, and engine failures
become `KeyUnwrapError::Engine`.

`block::unwrap_now` runs it through a synchronous `Aes128Engine`, which loads
a key for the run and erases it when its keyed view drops; the request's key
is erased right after the load, before the first block. A refused key or
block (`BlockError`) ends the operation as `KeyUnwrapError::Engine`. Every infallible `Aes128Blocks`
primitive (software, its `BlockCipher` blocks) is an engine that never
refuses; a hardware engine implements `Aes128Engine` itself.
`BlockKeyUnwrap` owns an engine and runs `unwrap_now`, so its `KeyUnwrap`
operation is ready on the first poll. `wrap` uses the infallible primitive.

With `test-support`, `conformance::key_unwrap_rfc3394` runs an engine through
the RFC 3394 §4.1 vector, a changed ciphertext, two invalid lengths and a
request after a refusal, driving a pending operation through a caller-supplied
step.

The crate is sans-IO: no unsafe code, allocation or runtime panic path. It
denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and
disallowed-macro Clippy lints, and its local Clippy configuration includes the
common bans and the complete sans-IO catalogue.
