# HIL harness

`oer-hil-harness` is the life cycle of a HIL scenario program. A scenario
names its image (`oer_image_build::images`) and the keys of the services that
image must serve, and lists its cases; `main` then:

1. builds the image through the image pipeline, before it waits for the stand;
2. leases the board (`--board`, or the one board of the image's chip with the
   `dut` role) from the arbiter;
3. writes the image unless the board's receipt says it runs it, and records
   the write in the board journal;
4. opens the board's console with a reset and starts the link;
5. checks that the running image is the built one (Hello's `image_crc32`
   against the bundle's runtime CRC-32, also after a reboot) and that its key
   pages hold every required key;
6. runs the cases (`--case` selects some, `--list` prints them), printing
   each outcome as soon as it is known;
7. finishes under the same lease: a device that runs the built image and left
   no request unconfirmed is clean; otherwise the board's reset ladder from
   the stand file runs (`oer_lab_board::reset::climb`), and after each rung
   the console is opened again and the image's Hello is awaited. The first
   rung after which the image answers recovers the board; a ROM that answers
   while the image does not leaves the board in service, since the stand
   can write it again; only a ROM that stays silent after the whole ladder
   quarantines the board through the arbiter. A cancelled program judges
   nothing, and the next session resets the board anyway;
8. releases the link, the board and the lease in that order, also on an error,
   a cancellation or a stage of opening that failed.

A case talks to the device through `Cx::call` (errors become the case's
failure) or `Cx::request` (errors as they came) and returns `check` results.
A refusal, a payload of another type or a reboot fails the case; a request
that was not sent, a request without a response, a broken link or a
cancellation leaves it incomplete, and an unconfirmed request leaves the rest
of its boot incomplete. The program prints the device's diagnostic lines and
how many were lost, then the cleanup and the summary, and exits 0 when every
case passed, 1 when one failed and 2 when one could not finish and none
failed. A passing scenario whose cleanup was not clean exits 2; a recovery
never turns a failure into a pass. It stores nothing.

A program killed outright leaves no child running (the `oer-os-process`
guardian), the arbiter powers the board's hub port back for the next holder,
and the next session resets the board and checks its image before any case.

```console
cargo test -p oer-hil-harness
```
