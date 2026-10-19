#![no_std]
use oer_ieee80211_lower_mac::{BeaconTimingPhase, RoleKeys, TsfPermission};
pub fn sample<'p, 'id: 'p, P: BeaconTimingPhase<'p, 'id>>(
    phase: &mut P,
    permission: TsfPermission<'id, P::Role, <P::Role as RoleKeys<'id, P::Types>>::CommandPath>,
) {
    phase.sample_tsf(permission);
    #[cfg(ui_reject)]
    phase.sample_tsf(permission); // ui-error: E0382
}
