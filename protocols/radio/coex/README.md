# Radio coexistence contract

`oer-radio-coex` defines the values every protocol and backend shares when
several radio protocols use one RF path:

- `RadioClient` names the protocol that joins the shared radio (Wi-Fi,
  Bluetooth LE, IEEE 802.15.4).
- `Operation` is implemented by each protocol port's own operation enum
  (beacon, acknowledgement, connection event, CSL slot, ...); its `AirClass`
  says whether the operation is served by hardware priority alone or has a
  hard deadline.

There is no urgency level. The chip family's coexistence owner, such as
[`oer-espressif-coex`](../../../chips/espressif/coex/README.md), maps a client
and an operation to its event numbers, hardware priorities, request kinds and
status words, which stay below the backend.

The crate owns no hardware and performs no arbitration. No supported backend
reports an RF grant or denial for an operation served by priority alone, so
no grant outcome is defined.
