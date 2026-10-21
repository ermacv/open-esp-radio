# Fixture installation

`oer-tool-lab-fixture-install` installs the versioned Linux fixture software
(`cargo stand fixture install`) and holds the fixed launchers the runner
starts it through.

It is a package of its own because its dependency closure is the
root-executed surface: the privileged `open-radio-fixture-install` binary and
the launchers (`tools/lab/fixture-install`) link only this package, so nothing else of the stand (SSH, the
router, the boards) reaches root through it.

Updates classify the installed artifact set from the receipt of the selected
generation. Roles added by the new bundle must have absent stable paths; an
artifact already listed in the installed receipt must still be intact. Failed
updates restore the previous generation and remove newly introduced paths.
`cargo stand fixture install --provider linux-net --dry-run` checks this
classification without changing the installation or running privileged checks.
Sudoers presence, ownership and mode remain mandatory checks in root apply;
the preview lists them as pending because the policy directory can be private.
