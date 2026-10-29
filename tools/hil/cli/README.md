# oer-tool-hil-cli

`cargo hil`. A HIL scenario is a workspace package with
`[package.metadata.open-radio] hil-scenario = true`; its name drops the
`oer-hil-scenarios-` prefix. The tool knows no scenario, image or result.

```console
cargo hil list
cargo hil run aes --list
cargo hil run aes --board dut --case reference-wraps
```

`run NAME ARGUMENTS` builds and runs the scenario's program with the
arguments that follow; its exit code is the scenario's verdict (0 pass,
1 fail, 2 incomplete).

```console
cargo test -p oer-tool-hil-cli
```
