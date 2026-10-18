#[cfg(feature = "wifi")]
#[test]
fn association_policy_and_wire_codec_share_the_same_types() {
    use oer::ieee80211::sta::association::{PhyMode, Preference, select_phy};

    let preference: mac::station::association::Preference = Preference::PreferHe20;
    let mode: mac::station::association::PhyMode = select_phy(preference, true, true, true);
    assert_eq!(mode, PhyMode::He20);
}

#[cfg(feature = "esp32s31")]
#[test]
fn chip_namespace_preserves_hal_type_identity() {
    use oer::chips::esp32s31::hal;

    let _: fn(chip_hal::root::RadioHardware) -> hal::root::RadioHardware = |owner| owner;
}

#[cfg(feature = "bluetooth")]
#[test]
fn bluetooth_facade_preserves_hci_and_link_layer_type_identity() {
    use oer::bluetooth::{hci as api, le::ll};

    let _: fn(hci::LeDtmPhy) -> api::LeDtmPhy = |phy| phy;
    let _: fn(le_ll::LeDeviceAddress) -> ll::LeDeviceAddress = |address| address;
}

#[cfg(feature = "i")]
#[test]
fn ieee802154_facade_preserves_channel_type_identity() {
    let _: fn(ieee802154::Channel) -> oer::ieee802154::Channel = |channel| channel;
}

#[cfg(feature = "esp32s31-bluetooth")]
#[test]
fn bluetooth_backend_preserves_controller_event_cell_identity() {
    let _: fn(
        chip_bluetooth::runtime_resources::ControllerEventCells,
    )
        -> oer::chips::esp32s31::driver::bluetooth::runtime_resources::ControllerEventCells =
        |cells| cells;
}

#[cfg(feature = "esp32s31-i")]
#[test]
fn chip_ieee802154_namespace_preserves_engine_type_identity() {
    let _: fn(
        chip_ieee802154::pib::Ieee802154PibDefaults,
    ) -> oer::chips::esp32s31::driver::ieee802154::pib::Ieee802154PibDefaults = |defaults| defaults;
}
