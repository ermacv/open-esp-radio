#![no_std]
use oer_ieee80211_sta::ftm::{FtmOperationId, FtmSession};

pub fn identity<const N: usize>(session: &FtmSession<N>) -> FtmOperationId {
    #[cfg(ui_reject)]
    {
        FtmOperationId { peer: session.operation().peer(), session_generation: session.operation().session_generation() } // ui-error: E0451
    }
    #[cfg(not(ui_reject))]
    session.operation()
}
