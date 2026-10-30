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
6. runs the cases (`--case` selects some, `--list` prints them);
7. releases the link, the board and the lease in that order, also on an error
   or a cancellation.

A case talks to the device through `Cx::call` (errors become the case's
failure) or `Cx::request` (errors as they came) and returns `check` results.
A refusal, a payload of another type or a reboot fails the case; a request
without a response, a broken link or a cancellation leaves it incomplete, and
an unconfirmed request leaves the rest of its boot incomplete. The program
prints each outcome and the device's diagnostic lines and exits 0 when every
case passed, 1 when one failed and 2 when one could not finish and none
failed. It stores nothing.

```console
cargo test -p oer-hil-harness
```
