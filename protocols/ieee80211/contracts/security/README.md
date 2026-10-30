# oer-ieee80211-contracts-security

The IEEE 802.11 installed-protection contract. `Ccmp128Key` is the one
owner of a CCMP-128 temporal key between RSN, the port, the software cipher
and a backend:

- construction copies borrowed material into the owner, never a by-value
  array;
- the bytes are only lent (`as_bytes`); there is no `Clone`, `Copy` or owned
  extraction, and `Debug` prints `Ccmp128Key(..)`;
- an ordinary drop zeroizes the owner's storage; a quarantined owner is not
  dropped, so its key stays in memory until the chip resets.

The codecs keep the syntax of the protection fields (CCMP header, packet
numbers, key IDs); this crate owns no format.
