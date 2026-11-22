# oer-hil-agent

The HIL agent: what every image that speaks the HIL protocol serves
besides the handlers of its services, a probe below the entry and a test
image on the `oer` entry alike. A probe differs from a scenario only in
what it is allowed to use; the work between the device and the host is
done through this agent. It stands below the entry, over the platform
runtime's USB console and records, so an image hands it what it has: its
boot identity, what it read before its boot (`Records`) and the value a
console fault ends it with.

```rust
static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    // ... the example's boot ...
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let records = Records {
        reset_cause: evidence.reset_cause,
        previous_panic: evidence.previous_panic,
        previous_exception: evidence.previous_exception.map(|record| (record.trap, record.observed)),
    };
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(move || {
        serve(console, oer::boot_id(), records, IMAGE, outbox, Error::Console, move || worker(inbox))
    }))
}

async fn worker(mut inbox: Inbox<'static, Request>) -> AppResult {
    loop {
        let (identity, request) = inbox.next().await;
        answer(identity, &handle(request).await);
    }
}
```

A probe opens the platform runtime's console itself, draws its boot
identity from its entropy owner and hands its own records and its own
failure value.

| Part | What it does |
| --- | --- |
| `serve` | Runs the HIL console over the platform runtime's USB console and the image's worker side by side until the chip resets. It takes the worker as a function that makes its future, so the image's future holds that future once, inside the select. The console answers the base module: Hello with the image's keys, its largest payload and the stage-two payload CRC-32, the boot evidence and link health. A transport fault ends it with the image's `console_fault` value. |
| `Mailbox` | Room for one request between the console and the worker. The console hands a request over without waiting; a second one while the worker has not taken the first is answered `Rejected(Busy)`. |
| `answer` | Publishes a response to its request without waiting. |
| `answer_then_trap` | Publishes the answer of a fault request, waits until the console wrote it and traps on an illegal instruction (`oer_portable_riscv::illegal_instruction`). A write the console cannot confirm (its barrier's exhausted fan-out, a terminal fault by its contract) returns the image's `console_fault` value without a trap; the worker ends with it. |

The boot identity and the records are the image's. An image on the
`oer` entry hands the library's (`oer::boot_id`, `Boot::evidence`):
hart 0's reset cause, with its raw code, the previous boot's panic, and the
previous boot's CPU exception as the post-mortem (`Fault::Exception`, one
boot, no checkpoints): the trap's registers as the runtime retained them
before the stop, apart from what the exception hook read after it. The
library printed and cleared each record before the boot; checkpoint pages
are empty. A probe hands what its own adoption read.

The console holds eight notes and eight messages in `critical_data`;
until the image opens it, notes go through the early USB writer, and what
that writer dropped is reported once the console runs.
