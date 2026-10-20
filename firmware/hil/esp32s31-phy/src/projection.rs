//! Read-only packing of the existing verification projection.
//! SOURCE: `firmware/probes/esp32s31-radio/src/calibration_projection.rs`;
//! reviewed field relation: `chips/esp32s31/knowledge/src/phy_relation/committed.rs`.

use oer_esp32s31_phy::{PhyCalibrationSnapshot, PhyState};

pub const SCHEMA: u16 = 1;
pub const WORDS: usize = 92;

pub fn capture(state: &PhyState, snapshot: &PhyCalibrationSnapshot) -> [u16; WORDS] {
    let mut words = [0; WORDS];
    let calibration = state.calibration_tracking_parameters(None);
    words[..5].copy_from_slice(&[
        calibration.current_temperature as u16,
        calibration.common_reference_temperature as u16,
        calibration.transmit_reference_temperature as u16,
        calibration.current_channel,
        u16::from(calibration.channel_bandwidth),
    ]);
    for (dst, src) in words[5..17]
        .iter_mut()
        .zip(snapshot.wifi.tx_dco[..3].iter().flatten())
    {
        *dst = *src;
    }
    for (dst, src) in words[17..29]
        .iter_mut()
        .zip(snapshot.bluetooth.tx_dco.iter().flatten())
    {
        *dst = *src;
    }
    for (dst, src) in words[29..77].iter_mut().zip(
        snapshot
            .wifi
            .wifi_index_dc
            .iter()
            .flatten()
            .chain(snapshot.wifi.wifi_fine_dc.iter().flatten())
            .chain(snapshot.wifi.shared_index_dc.iter().flatten()),
    ) {
        *dst = *src;
    }
    let power = state.tx_power_tracking_parameters(true);
    words[77..84].copy_from_slice(&[
        power.previous_tracking_temperature as u16,
        power.previous_tracking_gain_base as i16 as u16,
        power.wifi_gain_base as i16 as u16,
        power.bluetooth_ieee802154_gain_base as i16 as u16,
        state.channel_parameters().tx_gain_adjustment as i16 as u16,
        match state.wifi_i2c_tracking_parameters().previous_band {
            oer_esp32s31_phy::tracking::PhyWifiI2cTrackingBand::Nominal => 0,
            oer_esp32s31_phy::tracking::PhyWifiI2cTrackingBand::Cold => 1,
            oer_esp32s31_phy::tracking::PhyWifiI2cTrackingBand::Elevated => 2,
            oer_esp32s31_phy::tracking::PhyWifiI2cTrackingBand::Hot => 3,
        },
        state.rfpll_reference_temperature() as u16,
    ]);
    for (dst, pair) in words[84..88]
        .iter_mut()
        .zip(snapshot.common.dcode.chunks_exact(2))
    {
        *dst = u16::from_le_bytes([pair[0], pair[1]]);
    }
    words[88] = u16::from_le_bytes([
        if snapshot.wifi.rx_gain_dc_calibrated {
            0x80
        } else {
            0
        },
        if snapshot.wifi.rx_gain_tables_initialized {
            2
        } else {
            0
        },
    ]);
    words[89] = u16::from_le_bytes([
        snapshot.wifi.shared_rx_table_last_index,
        snapshot.wifi.wifi_rx_table_last_index,
    ]);
    // Tracking progress is not retained and is excluded by the reviewed relation.
    words[90] = 0;
    words[91] = u16::from(snapshot.common.sensor_index);
    words
}
