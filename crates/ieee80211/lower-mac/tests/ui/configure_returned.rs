#![no_std]
use oer_ieee80211_lower_mac::{
    Configured, LowerMacEvent, LowerMacPhase, LowerMacTerminal, StationPhase, StationReceive,
};
pub async fn reuse<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P) {
    if let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(Configured::Station(
        permission,
    ))))) = phase.next_event().await
    {
        phase.station().configure(permission, StationReceive::Off);
        #[cfg(ui_reject)]
        phase.station().configure(permission, StationReceive::Off); // ui-error: E0382
    }
}
