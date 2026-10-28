# Linux fixture contract

`oer-lab-fixture` is the library of the Linux fixture software of the HIL
stand: its versioned provisioning (`cargo stand fixture install`), the fixed
launchers' admission, and the requests and reports of the finite helpers
`open-radio-bluetooth` and `open-radio-probe`.

The programs are packages without a library, so the contract has one
definition: `tools/lab/fixture-install` (the privileged
`open-radio-fixture-install` and the fixed launchers) and `tools/hil/fixture`
(the finite helpers). The privileged programs link only this library and
`oer-os-process`, without its `linux-socket` feature; nothing else of the
stand (SSH, the router, the boards) reaches root through them. The library's
dependencies are foundation crates and `oer-hil-protocol`.

Updates classify the installed artifact set from the receipt of the selected
generation. Roles added by the new bundle must have absent stable paths; an
artifact already listed in the installed receipt must still be intact. Failed
updates restore the previous generation and remove newly introduced paths.
`cargo stand fixture install --provider linux-net --dry-run` checks this
classification without changing the installation or running privileged checks.
Sudoers presence, ownership and mode remain mandatory checks in root apply;
the preview lists them as pending because the policy directory can be private.
