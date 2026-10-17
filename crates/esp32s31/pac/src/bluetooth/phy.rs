//! The BLE PHY ETM route owner.
//!
//! The vendor's BLE PHY register initialization routes and enables the PHY
//! ETM channel; Direct Test Mode disables that route for a test without CTE
//! and restores it afterwards. The initialization body itself is recorded as
//! facts in `knowledge/esp32s31/bluetooth/` and rewritten from them, not
//! carried as code (source policy, sources without a license).

use crate::BluetoothTaskRegisters;

/// Owner of a disabled BLE PHY ETM route. Only
/// [`BluetoothTaskRegisters::restore_ble_phy_etm_route`] consumes it, so a
/// path that loses it leaves visible evidence rather than a silently
/// unrouted PHY.
#[derive(Debug)]
#[must_use = "a disabled BLE PHY ETM route must be restored"]
pub struct BlePhyEtmRouteDisabled {
    _private: (),
}

impl BluetoothTaskRegisters {
    /// Disable the BLE PHY ETM route for one Direct Test Mode event.
    ///
    /// SOURCE: pinned `libble_app.a[dtm_4.o]` event bodies
    /// `sym_dtm_2zeOUjc7g55zkDNQuZkg` and
    /// `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` disable, for a test without CTE,
    /// the vendor channel carrying the route that PHY register initialization
    /// installs; this owner carries that route on channel two.
    pub fn disable_ble_phy_etm_route(&mut self) -> BlePhyEtmRouteDisabled {
        self.etm.disable();
        BlePhyEtmRouteDisabled { _private: () }
    }

    /// Route and enable the BLE PHY ETM channel again after a test.
    ///
    /// SOURCE: pinned Test End `sym_dtm_NsbldBIeGraE2wg0AVy7` calls
    /// `r_ble_phy_init`, which leads to the PHY register initialization
    /// that routes and enables the channel.
    pub fn restore_ble_phy_etm_route(&mut self, route: BlePhyEtmRouteDisabled) {
        let BlePhyEtmRouteDisabled { _private: () } = route;
        self.etm.route_and_enable();
    }
}
