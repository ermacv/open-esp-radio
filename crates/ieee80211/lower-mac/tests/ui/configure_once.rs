#![no_std]
use oer_ieee80211_lower_mac::{
    ConfigurePermission, PhaseTypes, Station, StationPhase, StationReceive,
};
pub fn configure<'p, 'id: 'p, P: StationPhase<'p, 'id>>(
    phase: &mut P,
    permission: ConfigurePermission<
        'id,
        Station,
        <P::Types as PhaseTypes<'id>>::StationCommandPath,
        <P::Types as PhaseTypes<'id>>::StationConfigureContext,
    >,
) {
    phase.configure(permission, StationReceive::Off);
    #[cfg(ui_reject)]
    phase.configure(permission, StationReceive::Off); // ui-error: E0382
}
