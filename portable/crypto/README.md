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

The RFC 3394 algorithm has one copy, in `rfc3394`. `Rfc3394Unwrap::next_block`
issues a secret `CipherBlock` input and an affine `Rfc3394Pending` continuation.
The engine returns an output or `BlockError` through `complete`; only then does
the mode advance. A pending continuation and each block erase their secret
state on drop. There is no async code inside the machine. Engine failures
become `KeyUnwrapError::Engine` and clear the response buffer.

`BlockKeyUnwrap` decrypts every issued block immediately through the synchronous
`BlockCipher`, so its operation is ready on the first poll; `Aes128Blocks`
loads a key for the run and erases it when its keyed view drops. `wrap` uses
the same synchronous primitive. Hardware engines can instead yield between
submission and completion without copying the mode algorithm.

With `test-support`, `conformance::key_unwrap_rfc3394` runs an engine through
the RFC 3394 §4.1 vector, a changed ciphertext, two invalid lengths and a
request after a refusal, driving a pending operation through a caller-supplied
step.

The crate is sans-IO: no unsafe code, allocation or runtime panic path. It
denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and
disallowed-macro Clippy lints, and its local Clippy configuration includes the
common bans and the complete sans-IO catalogue.
