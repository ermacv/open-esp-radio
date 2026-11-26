#![no_std]
use oer_protocols_ieee80211_contracts_port::{InterfaceLayout, RadioLayout, RadioPort, Station};
use oer_protocols_ieee80211_testing_model::{
    MODEL_AGREEMENTS, MODEL_CHAIN_SLOTS, MODEL_CHAINS, ModelPort, StationModel,
};
// A port serves the layouts its `RadioPort` is implemented for: a layout
// of group key slots the model does not have does not build.
type ThreeGroups = RadioLayout<
    MODEL_CHAINS,
    MODEL_CHAIN_SLOTS,
    (InterfaceLayout<Station, 8, 1, 3, MODEL_AGREEMENTS>,),
>;
fn served<'id, P: RadioPort<'id>>() {}
pub fn station() {
    served::<ModelPort<'static, StationModel<8>>>();
    #[cfg(ui_reject)]
    served::<ModelPort<'static, ThreeGroups>>(); // ui-error: E0277
}
