# oer-hil-agent-espressif

The HIL agent on the `oer-espressif` application entry. A HIL test image
builds its drivers and services through the same `main!(boot)` as the
example of its function; it adds the SDK's `console` feature, settings in
place of the library's defaults where the test needs them, and the handlers
of its services. This crate holds what every such image serves besides its
handlers.

```rust
static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

fn boot(p: Peripherals, mut boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    // ... the example's boot ...
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(serve(console, evidence, IMAGE, outbox, move || worker(inbox))))
}

async fn worker(mut inbox: Inbox<'static, Request>) -> Infallible {
    loop {
        let (identity, request) = inbox.next().await;
        answer(identity, &handle(request).await);
    }
}
```

| Part | What it does |
| --- | --- |
| `serve` | Runs the HIL console over the library's USB console and the image's worker side by side until the chip resets. It takes the worker as a function that makes its future, so the application's future holds that future once, inside the select. The console answers the base module: Hello with the image's keys, its largest payload and the stage-two payload CRC-32, the boot evidence and link health. A transport fault ends the application with `Error::Console`. |
| `Mailbox` | Room for one request between the console and the worker. The console hands a request over without waiting; a second one while the worker has not taken the first is answered `Rejected(Busy)`. |
| `answer` | Publishes a response to its request without waiting. |
| `answer_then_trap` | Publishes the answer of a fault request, waits until the console wrote it and traps on an illegal instruction (`oer_riscv::illegal_instruction`); without a confirmed write it returns the console's error and does not trap. |

The boot identity is the library's (`oer_espressif::boot_id`). The boot
evidence is what the library read before the boot (`Boot::evidence`):
hart 0's reset cause, with its raw code, the previous boot's panic, and the
previous boot's CPU exception as the post-mortem (`Fault::Exception`, one
boot, no checkpoints): the trap's registers as the runtime retained them
before the stop, apart from what the exception hook read after it. The
library printed and cleared each record before the boot; checkpoint pages
are empty.

The console holds eight notes and eight messages in `critical_data`;
until the image opens it, notes go through the early USB writer, and what
that writer dropped is reported once the console runs.
