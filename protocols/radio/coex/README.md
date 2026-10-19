# Radio coexistence contract

`oer-radio-coex` defines the values every protocol and backend shares when
several radio protocols use one RF path:

- `RadioClient` names the protocol that joins the shared radio (Wi-Fi,
  Bluetooth LE, IEEE 802.15.4).
- `CoexPriority` orders how urgently one operation needs the antenna, from
  `Idle` to `Critical`, relative to its client.

Protocol levels convert into this vocabulary: the Bluetooth LE
`CoexistenceLevel` (`Baseline`, `Elevated`, `Critical`) and the Espressif
IEEE 802.15.4 levels (`Idle`, `Low`, `Middle`, `High`). Backends convert
back into their own arbitration values, such as the Espressif coexistence
event numbers, request kinds and status words of
[`oer-espressif-coex`](../../../chips/espressif/coex/README.md), which stay
below the backend.

The crate owns no hardware and performs no arbitration. No supported backend
reports an RF grant or denial, so no grant outcome is defined.
