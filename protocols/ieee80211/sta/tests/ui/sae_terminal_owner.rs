#![no_std]
use oer_ieee80211_sta::join::sae::{StaSaeAuthentication, StaSaeEvent};
use oer_time::Instant;

pub fn observe(exchange: StaSaeAuthentication, frame: &[u8], now: Instant) {
    let event = exchange.observe_management_frame(frame, now);
    #[cfg(ui_reject)]
    let _ = exchange.on_deadline(now); // ui-error: E0382
    #[cfg(not(ui_reject))]
    if let StaSaeEvent::Irrelevant(exchange) = event {
        let _ = exchange.on_deadline(now);
    }
}
