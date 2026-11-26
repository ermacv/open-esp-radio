// The SDK RPC image: the library's boot with the AES engine and the
// console, the HIL agent serving the base module and the AES service, and
// the engine that answers the AES requests, one at a time.

use oer::{Boot, Console, Error, Peripherals, Started};
use oer_hil_agent::{Records, Writer};
use oer_hil_services_aes::{Engine, Request};
use static_cell::ConstStaticCell;

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
    let writer = WRITER.try_take().ok_or(Error::Storage)?;
    Ok(boot.run(move || async move {
        // The AES service is commands alone: nothing observes, and the
        // engine commands, each unwrap to completion on the library's
        // synchronous AES engine.
        let mut commander = Engine::new(engine, |engine: &oer::AesEngine| {
            engine.last_stop().map(|stop| match stop {
                oer::AesStop::Timeout { observation } => {
                    oer_hil_services_aes::EngineObservation::Timeout { observation }
                }
                oer::AesStop::InvalidState { observation } => {
                    oer_hil_services_aes::EngineObservation::InvalidState { observation }
                }
            })
        });
        oer_hil_agent::serve::<_, Request, _, _, _>(
            console,
            oer::boot_id(),
            records,
            services::IMAGE,
            oer_hil_agent::Mount {
                writer,
                observer: &mut (),
                commander: &mut commander,
            },
            Error::Console,
        )
        .await
    }))
}
