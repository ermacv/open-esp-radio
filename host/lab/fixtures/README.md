# Fixture installation

`oer-lab-fixtures` installs the versioned Linux fixture software
(`cargo stand fixture install`) and holds the fixed launchers the runner
starts it through.

It is a package of its own because its dependency closure is the
root-executed surface: the privileged `open-radio-fixture-install` binary and
the launchers (`tools/fixture-install`) link only this package, so nothing else of the stand (SSH, the
router, the boards) reaches root through it.
