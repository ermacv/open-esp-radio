# oer-portable-crypto

The AES-128 key owner, the engine contract and the mode algorithms over it,
shared by every protocol that needs them.

An operation is a function over what its caller lends it for the call: a
synchronous AES-128 engine, the key (`Aes128Key`, which erases itself on
drop) and the buffer it works in place. No operation outlives the call, so
the engine, the key and the buffer are the caller's again on return.

The RFC 3394 algorithm has one copy, in `rfc3394`, over RFC 3394's own layout
in both directions: the integrity register's semiblock, then the registers,
which hold the key data. `rfc3394::unwrap` runs the index-based unwrap in
place over the whole buffer and answers the plaintext as the slice after the
first semiblock; `rfc3394::wrap` takes a semiblock of room, then the
plaintext, and leaves the whole buffer the ciphertext. Nothing moves, and the
buffer is split once, at admission. A length refusal loads no key and leaves
the buffer as it was; after admission, an integrity failure or the engine's
stop clears the whole buffer. The key is in the engine from its load to its
keyed view's drop, within the call, and the block is erased on every exit.
Errors are properties of the data (`InvalidLength`, `IntegrityCheckFailed`)
or the engine's stop (`Engine`).

The engine and its keyed view (`Aes128Engine`, `BlockEngine`) name what a
key or a block may answer besides its result, `Stop`:
`core::convert::Infallible` for a software engine, whose callers then have no
engine error path, and `EngineStopped` for one that may stop. The stop
carries nothing; what the engine observed is its driver's record.

`ccm` (RFC 3610, thirteen-byte nonce, MIC length checked at compile time) and
`cmac` (RFC 4493, `tag` and `verify` over a message given in pieces) run
over the same keyed `BlockEngine`, so a software block and a hardware
accelerator share one copy of each mode. A stop is the operation's `Engine`
error: CCM erases the body it was decrypting or encrypting, and a CMAC
computation ends with the call that holds it, so nothing of it is fed
again. Both
compare tags in constant time and erase their chaining values on every exit.
The host tests check them against RustCrypto's `ccm` and `cmac`, RFC 4493's
examples and a refusal at every block.

With `test-support`, `conformance::key_unwrap_rfc3394` runs an engine through
the RFC 3394 §4.1 vector, a changed ciphertext, two invalid lengths and an
unwrap after a refusal, each through `rfc3394::unwrap`.

The crate is sans-IO: no unsafe code, allocation or runtime panic path. It
denies unwrap, expect, panic, unreachable, todo, unimplemented,
indexing/slicing, arithmetic-side-effect, disallowed-method and
disallowed-macro Clippy lints, and its local Clippy configuration includes the
common bans and the complete sans-IO catalogue.
