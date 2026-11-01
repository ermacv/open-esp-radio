// The SDK RPC image: the library's boot with the AES service and the
// console, the HIL agent serving the base module and the AES service, and
// one worker that answers the AES requests in order.

use oer_espressif::{
    AES_CAPACITY, Aes, AesClient, AppResult, Boot, Console, Error, Peripherals, Started,
};
use oer_hil_agent_espressif::{Inbox, Mailbox, answer};
use oer_services_aes::{Handler as _, Request};
use static_cell::ConstStaticCell;

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

oer_espressif::entry!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let client = aes.client()?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(oer_hil_agent_espressif::serve(
        console,
        evidence,
        services::IMAGE,
        outbox,
        move || worker(client, inbox),
    )))
}

/// Answer each AES request in order through one client of the library's
/// AES service.
async fn worker(client: AesClient, mut requests: Inbox<'static, Request>) -> AppResult {
    let mut engine = oer_services_aes::Engine::<AesClient, AES_CAPACITY>::new(client);
    loop {
        let (identity, request) = requests.next().await;
        match request {
            Request::UnwrapKey(unwrap) => answer(identity, &engine.unwrap_key(unwrap).await),
        }
    }
}
