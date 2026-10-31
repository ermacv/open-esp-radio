# LAB client

`oer-lab-client` is how a command reaches the LAB service
([`oer-tool-lab-service`](../service/README.md)), the one owner of the stand's
leases and hardware: the IPC protocol ([`protocol`](src/protocol.rs)) and
the connection a command holds its lease by ([`client`](src/client.rs)).
`cargo fw`, `cargo hil`, `cargo stand` and the HIL harness depend on this
crate alone; the service's scheduler and drivers stay out of their builds.

- `Client::open` connects to the installed service and greets it; a service
  of another protocol refuses with its reason.
- `Client::acquire` queues for a lease and waits for the grant, which comes
  once the service prepared the lease's resources; a cancelled command stops
  waiting. Inside a command run under a lease (`OER_LAB_LEASE`) it joins
  that lease for the claims it names instead, which the token must allow.
  `Client::token` is the token to hand the commands this client runs: it
  allows what this client holds, no more.
- `Client::start` and `Client::run` run an operation: its output is a
  stream, its input flows by credit (`Input::send` waits for the operation
  to take what it sent before). One reader sorts the service's frames and
  never waits for an operation's reader: output beyond 1 MiB waiting for it
  is dropped and counted, and the replies behind it go on.
- `Client::release` finishes the lease: the service stops its operations,
  recovers its resources and answers once every recovery ended; an error
  names each resource left unavailable. Dropping a client without it
  releases the lease without waiting, and a client that dies leaves it
  abandoned; either way the service stops its operations first.

```console
cargo test -p oer-lab-client
```
