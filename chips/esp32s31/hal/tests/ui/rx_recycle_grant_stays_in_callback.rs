#![no_std]
use oer_esp32s31_hal::ieee80211::dma::rx_ring::RecycleGrant;

// The higher-ranked callback type used by the ring's recycle/prepare paths.
pub fn recycle(run: impl FnOnce(&mut dyn for<'ring> FnMut(RecycleGrant<'ring>))) {
    #[cfg(not(ui_reject))]
    run(&mut |grant| { let _ = grant.index(); });
    #[cfg(ui_reject)]
    {
        let mut retained = None;
        run(&mut |grant| retained = Some(grant)); // ui-error: E0521
        drop(retained);
    }
}
