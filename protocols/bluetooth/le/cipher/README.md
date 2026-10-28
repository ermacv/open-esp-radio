# Bluetooth LE cipher

`oer-bluetooth-le-cipher` provides allocation-free AES-CCM ACL packet protection,
LTK-to-session-key derivation and the 39-bit packet-counter/direction/IV nonce
transform. It depends on LE wire formats and the same RustCrypto AES/CCM and
zeroize primitives used by the Link Layer.

Callers supply the counter, direction, IV and header for each transform. The
crate owns no connection, procedure, replay frontier, packet counter or clock.
LL retains those responsibilities; a retransmission reuses retained ciphertext.

Payload extent checks are available before a caller selects its counter. Empty
LLID=01 PDUs bypass CCM and consume no nonce. MIC-only payloads are invalid.
Successful decryption clears MIC storage; MIC failure zeroizes the complete
supplied payload. LTKs, session keys, temporary key copies and AES schedules
zeroize on drop. Key and diversifier bytes use Bluetooth wire octet order;
nonces use nonce octet order. Counters outside 39 bits are refused before a
nonempty payload is modified.

Tests retain the Bluetooth Core Vol 6 Part C encryption samples for both
traffic directions, alongside buffer, nonce, authentication and zeroization
checks. Backend and model consumers can use this crate without LL or HCI.
