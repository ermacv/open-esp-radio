// The SDK RPC image: the library's boot with the AES engine and the
// console, the HIL agent serving the base module and the AES service, and
// one worker that answers the AES requests in order on the engine.

use oer::{AesEngine, AppResult, Boot, Console, Error, Peripherals, Started};
use oer_hil_agent::{Inbox, Mailbox, Records, Writer};
use oer_hil_services_aes::{Handler as _, Request};
use static_cell::ConstStaticCell;

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());
/// The console's state: only the console's task touches it.
static WRITER: ConstStaticCell<Writer> = ConstStaticCell::new(Writer::new());

oer::entry!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let engine = p.aes.engine(&mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let records = Records {
        reset_cause: evidence.reset_cause,
        previous_panic: evidence.previous_panic,
        previous_exception: evidence.previous_exception.map(|record| (record.trap, record.observed)),
    };
    let (mailbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    let writer = WRITER.try_take().ok_or(Error::Storage)?;
    Ok(boot.run(move || {
        oer_hil_agent::serve(
            console,
            oer::boot_id(),
            records,
            services::IMAGE,
            oer_hil_agent::Mount {
                console: mailbox,
                writer,
            },
            Error::Console,
            move || worker(engine, inbox),
        )
    }))
}

/// Answer each AES request in order, each to completion on the library's
/// synchronous AES engine.
async fn worker(
    engine: AesEngine,
    mut requests: Inbox<'static, Request>,
) -> AppResult {
    let mut engine = oer_hil_services_aes::Engine::new(engine);
    loop {
        let (request, reply) = requests.next().await;
        match request {
            Request::UnwrapKey(unwrap) => reply.answer(&engine.unwrap_key(unwrap).await),
        }
    }
}
