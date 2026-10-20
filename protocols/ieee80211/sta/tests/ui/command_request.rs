#![no_std]
use oer_ieee80211_port::Ieee80211Capabilities;
use oer_ieee80211_sta::machine::{StaRequest, StaTokens, Station};

pub fn submit<'id, T: StaTokens, K: Ieee80211Capabilities>(
    station: &mut Station<'id, T, K>,
    now: oer_time::Instant,
    request: StaRequest<'id, T::ConfigurationPath>,
) {
    let result = station.command(now, request);
    #[cfg(ui_reject)]
    let _correlation = request.correlation(); // ui-error: E0382
    #[cfg(not(ui_reject))]
    if let Err(refused) = result {
        let _original = refused.request;
    }
}
