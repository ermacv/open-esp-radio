// Without a build profile the image reports NotConfigured and starts no
// RF: it takes no radio owner, so no radio interrupt is bound.
use oer_espressif::{Boot, Error, Peripherals, Started, println};

oer_espressif::entry!(boot);

fn boot(_: Peripherals, boot: Boot) -> Result<impl Started, Error> {
    Ok(boot.run(async {
        let _ = println!("OER-RESULT: not-configured wifi-station");
        core::future::pending().await
    }))
}
