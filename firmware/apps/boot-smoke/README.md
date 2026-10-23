# boot-smoke

The smallest ESP32-S31 staged application adopts the bootstrap's PSRAM
mapping, reports a valid panic left by the previous boot, then idles.

```console
cargo fw build boot-smoke
cargo fw run boot-smoke --board s31-c
```

The USB console carries [typed diagnostic records](../../../portable/diagnostics/README.md).
`cargo fw run` and `cargo fw monitor` render them on the host:

```text
OER-PANIC previous src/main.rs:42:7 on hart 0: example panic
OER-RESULT: pass boot-smoke
```

The panic line is present only when a retained record survived. A formatted
message is shown as `(formatted message not kept)`; an interrupt panic also
includes its interrupted PC. All retained fields and their order are kept.
The final typed verdict closes the sequence so loss of a previous-panic
record is an error. Target code performs no text formatting.

`target/fw/runs/<time>-boot-smoke/log.txt` is the rendered view;
`console.raw.txt` keeps the encoded console lines. Replay the latter with
`cargo fw decode-console <path>`. A plain serial terminal sees the encoded
`OER-DIAG:01…` lines. No panic handler or retained layout is changed.

The HIL scenario also named `boot-smoke` exercises its own dedicated HIL
image and timer rearm; it is separate from this minimal app.
