#![no_std]
use oer_ieee80211_core_roles_sta::machine::{ConnectCommand, StaCommandCredit, StaRequest};
use oer_radio_port::PoolPath;

pub fn start_once<'id, Path: PoolPath>(credit: StaCommandCredit<'id, Path, ConnectCommand>) {
    let first = StaRequest::new(1, credit);
    #[cfg(ui_reject)]
    let duplicate = StaRequest::new(2, credit); // ui-error: E0382
    drop(first);
}
