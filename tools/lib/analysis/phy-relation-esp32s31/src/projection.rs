//! Production calibration words in the order the vendor comparison reads
//! them: the calibration projection of the tracking roots and their
//! committed state words.
//!
//! The comparison probes publish these words for the Blobray tracking
//! scenario, and the hardware calibration cross-check projects a snapshot
//! captured on the board through the same functions. The file depends only
//! on `oer_esp32s31_phy`.

/// Committed calibration state in the vendor `phy_param` byte order: the eight
/// DCODE codes, the two status bytes holding the RX-gain DC (0x80 of the
/// first) and RX-gain table (0x02 of the second) completion flags, the
/// shared and Wi-Fi RX-gain table last indices, the tracking progress, then the
/// temperature-sensor index.
pub fn snapshot_committed(
    state: &oer_esp32s31_phy::PhyState,
    progress: u16,
    output: &mut [u16; 8],
) {
    let snapshot = oer_esp32s31_phy::validation::calibration_snapshot(state);
    let dcode = snapshot.common.dcode;
    for (destination, pair) in output[..4].iter_mut().zip(dcode.chunks_exact(2)) {
        *destination = u16::from_le_bytes([pair[0], pair[1]]);
    }
    output[4] = u16::from_le_bytes([
        if snapshot.wifi.rx_gain_dc_calibrated {
            0x80
        } else {
            0
        },
        if snapshot.wifi.rx_gain_tables_initialized {
            0x02
        } else {
            0
        },
    ]);
    output[5] = u16::from_le_bytes([
        snapshot.wifi.shared_rx_table_last_index,
        snapshot.wifi.wifi_rx_table_last_index,
    ]);
    output[6] = progress;
    output[7] = u16::from(snapshot.common.sensor_index);
}

/// Words of [`snapshot_calibration`]: references, channel, TX DC rows, then
/// the RX DC banks.
pub const CALIBRATION_WORDS: usize = 77;

pub fn snapshot_calibration(
    state: &oer_esp32s31_phy::PhyState,
    output: &mut [u16; CALIBRATION_WORDS],
) {
    let parameters = state.calibration_tracking_parameters(None);
    output[..5].copy_from_slice(&[
        parameters.current_temperature as u16,
        parameters.common_reference_temperature as u16,
        parameters.transmit_reference_temperature as u16,
        parameters.current_channel,
        u16::from(parameters.channel_bandwidth),
    ]);
    let wifi_dc = state.tx_dc_pwdet_parameters().dco;
    let bt_dc = state.bluetooth_tx_gain_parameters().seed;
    for (destination, source) in output[5..17].iter_mut().zip(wifi_dc.iter().flatten()) {
        *destination = *source;
    }
    for (destination, source) in output[17..29].chunks_exact_mut(2).zip(bt_dc) {
        destination.copy_from_slice(&[source as u16, (source >> 16) as u16]);
    }
    let rx = state.rx_gain_memory_parameters();
    for (destination, source) in output[29..].iter_mut().zip(
        rx.wifi_index_dc
            .iter()
            .flatten()
            .chain(rx.wifi_fine_dc.iter().flatten())
            .chain(rx.shared_index_dc.iter().flatten()),
    ) {
        *destination = *source;
    }
}

/// Words of [`snapshot_parent`].
pub const PARENT_WORDS: usize = 7;

/// The parent root's retained power, gain and RFPLL words: the power
/// tracking temperature and gain cache, the Wi-Fi and Bluetooth gain bases,
/// the retained gain adjustment, the Wi-Fi I2C tracking band and the RFPLL
/// reference temperature.
pub fn snapshot_parent(state: &oer_esp32s31_phy::PhyState, output: &mut [u16; PARENT_WORDS]) {
    use oer_esp32s31_phy::tracking::i2c::PhyWifiI2cTrackingBand;
    let power = state.tx_power_tracking_parameters(true);
    output[..4].copy_from_slice(&[
        power.previous_tracking_temperature as u16,
        power.previous_tracking_gain_base as i16 as u16,
        power.wifi_gain_base as i16 as u16,
        power.bluetooth_ieee802154_gain_base as i16 as u16,
    ]);
    output[4] = state.channel_parameters().tx_gain_adjustment as i16 as u16;
    output[5] = match state.wifi_i2c_tracking_parameters().previous_band {
        PhyWifiI2cTrackingBand::Nominal => 0,
        PhyWifiI2cTrackingBand::Cold => 1,
        PhyWifiI2cTrackingBand::Elevated => 2,
        PhyWifiI2cTrackingBand::Hot => 3,
    };
    output[6] = oer_esp32s31_phy::validation::rfpll_reference_temperature(state) as u16;
}
