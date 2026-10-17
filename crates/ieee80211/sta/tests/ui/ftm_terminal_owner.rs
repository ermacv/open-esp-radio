#![no_std]
use oer_ieee80211_sta::ftm::FtmTerminal;
pub fn result<const N: usize>(terminal: FtmTerminal<N>) {
    let (_requester, _outcome) = terminal.into_parts();
    #[cfg(ui_reject)]
    let _again = terminal.into_parts(); // ui-error: E0382
}
