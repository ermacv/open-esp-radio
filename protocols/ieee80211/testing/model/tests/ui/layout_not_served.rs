#![no_std]
use oer_protocols_ieee80211_contracts_port::{InterfaceLayout, RadioLayout, RadioPort, Station};
use oer_protocols_ieee80211_testing_model::{
    MODEL_AGREEMENTS, MODEL_CHAIN_SLOTS, ModelLongs, ModelPort, StationModel,
};
// A port serves the layouts its `RadioPort` is implemented for: a layout
// of group key slots the model does not have does not build.
type ThreeGroups = RadioLayout<
    ModelLongs,
    MODEL_CHAIN_SLOTS,
    (InterfaceLayout<Station, 1, 3, MODEL_AGREEMENTS>,),
>;
fn served<'id, P: RadioPort<'id>>() {}
pub fn station() {
    served::<ModelPort<'static, StationModel>>();
    #[cfg(ui_reject)]
    served::<ModelPort<'static, ThreeGroups>>(); // ui-error: E0277
}
