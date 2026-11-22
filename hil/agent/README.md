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
static WRITER: ConstStaticCell<Writer> = ConstStaticCell::new(Writer::new());

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    // ... the example's boot ...
    let engine = p.aes.engine(&mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let records = Records {
        reset_cause: evidence.reset_cause,
        previous_panic: evidence.previous_panic,
        previous_exception: evidence.previous_exception.map(|record| (record.trap, record.observed)),
    };
    let writer = WRITER.try_take().ok_or(Error::Storage)?;
    Ok(boot.run(move || async move {
        // What commands: the service's commanding trait on the engine.
        let mut commander = Engine::new(engine);
        let mount = Mount { writer, observer: &mut (), commander: &mut commander };
        serve::<_, Request, _, _, _>(console, oer::boot_id(), records, IMAGE, mount, Error::Console)
            .await
    }))
}
```

A probe opens the platform runtime's console itself, draws its boot
identity from its entropy owner and hands its own records and its own
failure value.

| Part | What it does |
| --- | --- |
| `serve` | Runs the HIL console over the platform runtime's USB console until the chip resets, calling the image's handlers. The console answers the base module (Hello with the image's keys, its largest payload and the stage-two payload CRC-32, the boot evidence and link health) and the image's observations in the step that decoded them, and runs its commands one at a time. A transport fault ends it with the image's `console_fault` value. |
| `Writer`, `Mount` | The console's state, the image's place in ordinary memory since only the console's task touches it, and what observes and what commands, two values split once at the image's composition, handed to `serve` together (`Mount`). |
| `After::Trap` | For a service set that serves the fault service: its command returns `(answer, After::Trap(Trap))`, after whose write `serve` traps on an illegal instruction (`oer_portable_riscv::illegal_instruction`). An answer that is not written whole gives no trap. |

The boot identity and the records are the image's. An image on the
`oer` entry hands the library's (`oer::boot_id`, `Boot::evidence`):
hart 0's reset cause, with its raw code, the previous boot's panic, and the
previous boot's CPU exception as the post-mortem (`Fault::Exception`, one
boot, no checkpoints): the trap's registers as the runtime retained them
before the stop, apart from what the exception hook read after it. The
library printed and cleared each record before the boot; checkpoint pages
are empty. A probe hands what its own adoption read.

What the early USB writer dropped
before the console took the peripheral is reported once the console runs.
