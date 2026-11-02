// The SDK RPC image: the library's boot with the AES engine and the
// console, the HIL agent serving the base module and the AES service, and
// one worker that answers the AES requests in order on the engine.

use oer::{AesEngine, AppResult, Boot, Console, Error, Peripherals, Started};
use oer_hil_agent::{Inbox, Mailbox, answer};
use oer_hil_services_aes::{Handler as _, Request};
use static_cell::ConstStaticCell;

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

oer::entry!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let engine = p.aes.engine(&mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(move || {
        oer_hil_agent::serve(
        console,
        evidence,
        services::IMAGE,
        outbox,
        move || worker(engine, inbox),
    )
    }))
}

/// Answer each AES request in order, each to completion on the library's
/// synchronous AES engine.
async fn worker(engine: AesEngine, mut requests: Inbox<'static, Request>) -> AppResult {
    let mut engine = oer_hil_services_aes::Engine::new(engine);
    loop {
        let (identity, request) = requests.next().await;
        match request {
            Request::UnwrapKey(unwrap) => answer(identity, &engine.unwrap_key(unwrap).await),
        }
    }
}
