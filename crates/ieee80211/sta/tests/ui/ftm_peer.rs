#![no_std]
use oer_ieee80211_mac::address::IndividualAddress;
use oer_ieee80211_sta::ftm::{FtmRefusal, FtmRequester, FtmSession};
use oer_time::Instant;

pub fn start<const N: usize>(
    requester: FtmRequester<N>,
    peer: IndividualAddress,
    now: Instant,
) -> Result<FtmSession<N>, FtmRefusal<FtmRequester<N>>> {
    #[cfg(ui_reject)]
    {
        requester.start(peer.get(), now) // ui-error: E0308
    }
    #[cfg(not(ui_reject))]
    requester.start(peer, now)
}
