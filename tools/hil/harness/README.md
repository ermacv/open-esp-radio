# HIL harness

`oer-hil-harness` is the life cycle of a HIL scenario program. A scenario
(`Scenario::new`) names its image (`oer_image_build::images`) and the keys
of the services that image must serve, and lists its cases; `main` then:

1. builds the image through the image pipeline, before it waits for the stand;
2. leases the board (`--board`, or the one board of the image's chip with
   the `dut` role) from the LAB service
   ([tools/lab/service](../../lab/service/README.md)), which runs every
   operation on it;
3. has the service write the image unless the board's receipt says it runs
   it; the service records the write in the board journal;
4. opens the board's console with a reset and starts the link on it: the
   service streams the console through a local socket the link reads as its
   serial line;
5. checks that the running image is the built one (Hello's `image_crc32`
   against the bundle's runtime CRC-32, also after a reboot) and that its key
   pages hold every required key;
6. runs the cases (`--case` selects some, `--list` prints them), printing
   each outcome as soon as it is known;
7. finishes the device under the same lease: one that runs the built image
   and left no request unconfirmed is clean; otherwise the board's reset ladder from
   the stand file runs (`oer_lab_board::reset::climb`), and after each rung
   the console is opened again and the image's Hello is awaited. The first
   rung after which another boot of the image answers recovers the board
   (the new console answers also a device the rung did not restart); a ROM that answers
   while the image does not leaves the board in service, since the stand
   can write it again; a ROM that stays silent after the whole ladder is
   reported as needing a person. A cancelled program judges nothing;
8. releases the link and the lease in that order, also on an error, a
   cancellation or a stage of opening that failed. The service then parks
   the board in its ROM's download mode, or keeps it out of service when
   not even that works.

A case talks to the device through `Cx`:

| Method | Gives |
| --- | --- |
| `call`, `request` | A request's typed response; `call` turns errors into the case's failure, `request` returns them as they came |
| `events`, `event`, `take_message` | A cursor over the running boot's messages from now on, made before a request to see its events, also those before its response; it reads in order, past what its filter refused, and other cursors read on their own; `event::<M>` fails the case when no `M` came in a complete interval and leaves it incomplete when a message of the interval was lost |
| `completeness`, `health` | Whether a cursor's interval lost a message or ended with its boot; the link's counters |
| `reset` | A new boot of the same image through the first rung of the board's ladder and a reopened console; the boot seen last, also before a failed reset, is no reset |

A refusal, a payload of another type or a reboot fails the
case; a request that was not sent, a request without a response, a broken
link or a cancellation leaves it incomplete, and an unconfirmed request
leaves the rest of its boot incomplete. The program prints the device's
diagnostic lines and how many were lost, then the cleanup and the summary,
and exits 0 when every case passed, 1 when one failed and 2 when one could
not finish and none failed. A passing scenario whose cleanup was not clean
exits 2; a recovery never turns a failure into a pass. It stores nothing.

A program killed outright leaves no child running (the `oer-os-process`
guardian), the arbiter powers the board's hub port back for the next holder,
and the next session resets the board and checks its image before any case.

```console
cargo test -p oer-hil-harness
```
