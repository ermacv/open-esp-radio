// Without a build profile the image reports NotConfigured and starts no
// RF: it takes no radio owner, so no radio interrupt is bound.
use core::future::Future;
use oer_espressif::{AppResult, Boot, Error, Peripherals, Started, println};

oer_espressif::main!(boot);

fn boot(_: Peripherals, boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    Ok(boot.run(async {
        let _ = println!("OER-RESULT: not-configured wifi-station");
        core::future::pending().await
    }))
}
