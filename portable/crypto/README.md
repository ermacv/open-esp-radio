# oer-crypto

Owned cryptographic operation contracts and the mode algorithms behind them,
shared by every protocol that needs them.

One operation is one request. `KeyUnwrapRequest<N>` owns an AES-128
key-encryption key and a buffer of capacity `N`; `block::unwrap_now` runs it
to completion over a synchronous AES-128 engine the caller lends for the call
and returns `KeyUnwrapResponse<N>`, which carries the same buffer back: the
plaintext on success, cleared on refusal. No operation outlives the call, so
the engine is free again on return. Consuming the request erases the key;
`Aes128Key` and `KeyData` erase themselves on drop. Errors are properties of
the wrapped data (`InvalidLength`, `IntegrityCheckFailed`) or the engine's
(`KeyUnwrapError::Engine`).

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
refuses; a hardware engine implements `Aes128Engine` itself. `wrap` uses the
infallible primitive.

`ccm` (RFC 3610, thirteen-byte nonce, MIC length checked at compile time) and
`cmac` (RFC 4493, fed in pieces) run over the same keyed `BlockEngine`, so a
software block and a hardware accelerator share one copy of each mode. A
refused block is the typed error of the operation: CCM erases the body it
was decrypting or encrypting, and the first refusal poisons a CMAC. Both
compare tags in constant time and erase their chaining values on every exit.
The host tests check them against RustCrypto's `ccm` and `cmac`, RFC 4493's
examples and a refusal at every block.

With `test-support`, `conformance::key_unwrap_rfc3394` runs an engine through
the RFC 3394 §4.1 vector, a changed ciphertext, two invalid lengths and a
request after a refusal, each through `block::unwrap_now`.

The crate is sans-IO: no unsafe code, allocation or runtime panic path. It
denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and
disallowed-macro Clippy lints, and its local Clippy configuration includes the
common bans and the complete sans-IO catalogue.
