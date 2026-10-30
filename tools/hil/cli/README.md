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
1 fail, 2 incomplete). The program runs in a process group the tool owns:
SIGINT, SIGTERM or SIGHUP to the tool gives the group 60 seconds to finish
its session before it is killed, and a tool that dies any other way takes
the group with it.

```console
cargo test -p oer-tool-hil-cli
```
