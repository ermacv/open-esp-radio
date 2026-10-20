#![no_std]
use oer_ieee80211_sta::machine::{ConnectCommand, StaCommand, StaCommandCredit, StaRequest};
use oer_radio_port::PoolPath;

pub fn start<'id, Path: PoolPath>(credit: StaCommandCredit<'id, Path, ConnectCommand>) {
    #[cfg(ui_reject)]
    let request = StaRequest::<'id, Path>::new(1, StaCommand::Connect); // ui-error: E0277
    #[cfg(not(ui_reject))]
    let request = StaRequest::new(1, credit);
    drop(request);
}
