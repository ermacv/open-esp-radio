# Bluetooth GATT application

`oer-bluetooth-gatt-trouble` is a portable GATT application library over the
Trouble Host: an adapter binding the Host to the open Controller's HCI. The
Bluetooth HIL agents (`oer-hil-agent` with `secure-gatt` and the ESP32-S31
agent's `bluetooth-gatt` images) compose it with the production Controller;
it stays a library so that each firmware composes its own platform and
console. It is an `adapter` because the Trouble Host and its host tests bind
`embassy-time`.

- `gatt` serves the plaintext profile: one writable value, a fixed legacy
  advertising payload and value-only observations.
- `secure` (feature) adds LE Secure Connections Numeric Comparison, a bounded
  asynchronous bond store and encrypted attribute access.

The library owns no platform, executor, controller or console. Callers supply
the `trouble-host` stack, the HCI transport and, for `secure`, the Numeric
Comparison console. Host tests exercise the attribute tables, pairing policy
and bond store against the portable HCI controller model:

```console
cargo test -p oer-bluetooth-gatt-trouble --all-features
```

## Authenticated GATT profile

`security::gatt::run` requires LE Secure Connections Numeric Comparison with
`DisplayYesNo`; Just Works, passkey entry, OOB and legacy pairing are not
fallbacks. The pinned Host rejects SC peers offering keys shorter than 128 bits.

The caller owns an asynchronous `BondStore`, bounded `RamBondStore`, and affine
Numeric Comparison requests independently of the Host. Store insertion rejects
unauthenticated records, duplicates and capacity exhaustion without evicting
existing keys. RAM records live outside Host resources but disappear on reset
or power loss; no persistent backend is implemented. Importing authenticated
metadata does not prove the original pairing method: imported records must
come from trusted Numeric Comparison enrollment. Bond changes work without
Controller privacy; the Host fork issues resolving-list commands only when
privacy is explicitly enabled. This profile does not enable address privacy.

The `security::console` confirmation accepts only this reply format, with the
displayed boot identifier, request identifier and six-digit number:

```text
confirm <16-hex-boot> <request-id> <six-digit-number> yes
confirm <16-hex-boot> <request-id> <six-digit-number> no
```

Only an explicit matching `yes` confirms locally; the peer must also confirm.
Wrong or stale boot IDs, request IDs and numbers are rejected. Dropping a
request invalidates queued replies; transport loss never confirms it.

The service/characteristic UUIDs are `0xfff0`/`0xfff1`. Value reads/writes and
CCCD writes require authenticated encryption. Application authorization also
requires successful bond-store insertion; restoring a connection requires the
matching stored bond and authenticated encryption. Compound ATT value reads
cannot bypass this gate. Subscribed writes queue notifications containing the
new one-byte value; queueing is not proof of peer reception. Discovery remains
public, the value survives reconnects and subscriptions are connection-local.

A full store accepts known peers only. Lost keys, failed pairing or rejected
confirmation disconnect without silent re-enrollment or key replacement. Store
errors stop the application. Restarting the application requires a fresh Host
restored from the retained store, since a cancelled insert may already have
committed.

`security::epoch::run` owns one Host epoch and borrows the application's store
and comparison sequence. On a stop request or failure it drops application
producers and, if bootstrap Reset is outstanding, keeps the existing Host
runner alive until its response arrives. A cancelled or failed bootstrap Reset
cannot authorize a second Reset with the same opcode. It then drops Host
producers, obtains the Controller through the fork's `Stack::into_controller`,
and awaits a separate HCI Reset while draining old events. The result
preserves the primary stop cause and any secondary failure while draining
bootstrap. `ShutdownAction` requests owner retention when Reset is unproven,
checked cold close after an application/Host failure, or checked restart after
an explicit successful stop request. No ordinary application failure requests
a SoC reset or silent retry. This is not physical retirement: the caller must
complete Controller hardware release before a cold restart.

Host tests do not qualify pairing or bonded reconnect over RF.
