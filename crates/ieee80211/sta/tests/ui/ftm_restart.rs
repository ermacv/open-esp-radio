#![no_std]
use oer_ieee80211_sta::ftm::FtmRequester;
use oer_ieee80211_mac::address::IndividualAddress;
use oer_time::Instant;
pub fn start<const N: usize>(requester: FtmRequester<N>, peer: IndividualAddress, now: Instant) {
    let session = match requester.start(peer, now) { Ok(session) => session, Err(_) => return };
    #[cfg(ui_reject)]
    let _second = requester.start(peer, now); // ui-error: E0382
    #[cfg(not(ui_reject))]
    {
        let (requester, _outcome) = session.cancel().into_parts();
        let _second = requester.start(peer, now);
    }
}
