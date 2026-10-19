# Regulatory table generator

`cargo regdb <db.txt> <revision> <out.rs>` reads the `db.txt` of
[wireless-regdb](https://git.kernel.org/pub/scm/linux/kernel/git/wens/wireless-regdb.git)
(ISC licence, see the repository `NOTICE`) and writes the rules of the
countries listed in `COUNTRIES` (`src/main.rs`) to
`protocols/radio/regulatory/src/generated.rs`. `<revision>` is the commit of
`db.txt` and goes into the header of the output.

The output is formatted with `rustfmt`, generated: regenerate it, never edit it.

Frequencies become kHz, powers mBm (milliwatts converted, rounded). `AUTO-BW`
and `wmmrule=` are not used by the domain and are dropped; `NO-OUTDOOR`,
`NO-IR` and `DFS` are kept. Any other flag, an unknown country in the list or
an unparsable line stops the run.
