# In-process HCI transport

`oer-bluetooth-hci-transport` joins an HCI Host and the LE Controller of one
process through two bounded packet queues. It owns the waiting half of the HCI
boundary; the packet values, their validation and the Controller codecs are the
sans-IO [`oer-bluetooth-hci`](../../../../protocols/bluetooth/hci/src/lib.rs).

- `LeControllerHciResources` is the statically bounded storage of one HCI
  epoch, checked against the bootstrap profile, and splits into
  `LeControllerHciEndpoints`.
- `InProcessHciHostTransport` implements `bt_hci::transport::Transport` for
  `bt_hci::ExternalController` and Trouble; `LeHostAclCreditSender` returns
  ACL credits without an event.
- `InProcessHciControllerTransport` is the raw Controller half: receive,
  admission of commands past queued ACL data, publication, closure,
  retirement and restart of an epoch.

Every wait is cancellation-safe and backed by `embassy-sync` wakers; the crate
owns no executor, timer or allocator.
