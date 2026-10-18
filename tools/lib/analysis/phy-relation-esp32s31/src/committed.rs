//! Reviewed correspondence between the vendor `phy_param` object and the
//! committed calibration words production publishes.
//!
//! The production probes lay out the committed calibration as the
//! calibration projection (references, channel, TX DC rows and RX DC banks)
//! followed, after the parent root's power, gain and RFPLL words, by the committed state words
//! (DCODE codes, status bytes, RX-gain table last indices, tracking progress
//! and sensor index). The tracking scenario compares these fields between
//! the pinned vendor roots and the compiled probes; the hardware calibration
//! cross-check compares them between the vendor `phy_param` and a production
//! snapshot captured on the board.

/// The vendor data object these fields index.
pub const VENDOR_OBJECT: &str = "phy_param";

/// One committed `phy_param` field and its production output location.
#[derive(Clone, Copy, Debug)]
pub struct OutputField {
    pub name: &'static str,
    /// Byte offset in `phy_param`.
    pub parameter: u32,
    /// Byte offset in the production output.
    pub output: u32,
    pub width: u8,
    pub count: u32,
}

/// `phy_param` offsets of the calibration projection.
pub const CURRENT_TEMPERATURE: usize = 0;
pub const COMMON_REFERENCE: usize = 400;
pub const TRANSMIT_REFERENCE: usize = 72;
pub const PARAMETER_CHANNEL: usize = 284;
pub const PARAMETER_BANDWIDTH: usize = 287;
const WIFI_DC_ROWS: usize = 168;
const BLUETOOTH_DC_ROWS: usize = 260;
const WIFI_RX_DC: usize = 334;
const SHARED_RX_DC: usize = 436;
/// `phy_param` offsets of the committed calibration state: the eight codes
/// ROM `phy_dcode_cal_init` stores from 0x1a1, and the status word whose
/// 0x80 and 0x200 bits `phy_set_rx_gain_table` sets after RX-gain DC and
/// table completion.
pub const DCODE: usize = 0x1a1;
pub const CALIBRATION_STATUS: usize = 0xa4;
/// `phy_param` offsets of the shared and Wi-Fi RX-gain table last indices.
pub const SHARED_LAST_INDEX: usize = 288;
pub const WIFI_LAST_INDEX: usize = 289;
/// `phy_param` offset of the tracking progress word the tracking children
/// set and `phy_param_track_tot` clears and returns.
pub const TRACKING_PROGRESS: usize = 0x1e6;
/// `phy_param` byte of the temperature-sensor index ROM
/// `phy_tsens_temp_read_local` stores with each sample.
pub const SENSOR_INDEX: usize = 0x16;

/// `phy_param` offsets of the parent's retained power, gain and RFPLL
/// state: the power tracking temperature and gain cache, the Wi-Fi and
/// Bluetooth gain bases, the retained gain adjustment, the Wi-Fi I2C
/// tracking band (0 nominal, 1 cold, 2 elevated, 3 hot) and the RFPLL
/// reference temperature.
const POWER_TEMPERATURE: usize = 4;
const SHARED_CACHE: usize = 290;
const WIFI_GAIN_BASE: usize = 291;
const BLUETOOTH_GAIN_BASE: usize = 292;
pub const GAIN_ADJUSTMENT: usize = 434;
pub const I2C_BAND: usize = 77;
const RFPLL_REFERENCE: usize = 304;

/// Bytes of the calibration projection every root output starts with.
pub const CALIBRATION_BYTES: u32 = 154;
/// Bytes of the parent words after the calibration projection.
pub const PARENT_BYTES: u32 = 14;
/// Committed-state bytes every root output ends with.
pub const COMMITTED_BYTES: u32 = 16;
/// The committed field that counts tracking progress rather than
/// calibration.
pub const TRACKING_PROGRESS_FIELD: &str = "tracking-progress";

/// Fields of the calibration projection in production output order.
pub const CALIBRATION: [OutputField; 9] = [
    field("current-temperature", CURRENT_TEMPERATURE, 0, 2, 1),
    field("common-reference", COMMON_REFERENCE, 2, 2, 1),
    field("transmit-reference", TRANSMIT_REFERENCE, 4, 2, 1),
    field("channel", PARAMETER_CHANNEL, 6, 2, 1),
    field("bandwidth", PARAMETER_BANDWIDTH, 8, 1, 1),
    field("wifi-dc-rows", WIFI_DC_ROWS, 10, 2, 12),
    field("bluetooth-dc-rows", BLUETOOTH_DC_ROWS, 34, 2, 12),
    // Wi-Fi per-gain DC, then the five `phy_rxdc_fine_cal` pairs.
    field("wifi-rx-dc", WIFI_RX_DC, 58, 2, 26),
    field("shared-rx-dc", SHARED_RX_DC, 110, 2, 22),
];

/// Fields of the parent words, which follow the calibration projection in
/// the parent root's output. Signed bytes occupy the low byte of a word.
pub const PARENT: [OutputField; 7] = [
    field("power-temperature", POWER_TEMPERATURE, 154, 2, 1),
    field("shared-cache", SHARED_CACHE, 156, 1, 1),
    field("wifi-gain-base", WIFI_GAIN_BASE, 158, 1, 1),
    field("bluetooth-gain-base", BLUETOOTH_GAIN_BASE, 160, 1, 1),
    field("gain-adjustment", GAIN_ADJUSTMENT, 162, 1, 1),
    field("i2c-band", I2C_BAND, 164, 1, 1),
    field("rfpll-reference", RFPLL_REFERENCE, 166, 2, 1),
];

/// Fields of the committed state words starting at output byte `start`.
pub const fn committed(start: u32) -> [OutputField; 5] {
    [
        field("dcode", DCODE, start, 1, 8),
        field("calibration-status", CALIBRATION_STATUS, start + 8, 1, 2),
        field("rx-table-last-indices", SHARED_LAST_INDEX, start + 10, 1, 2),
        field(TRACKING_PROGRESS_FIELD, TRACKING_PROGRESS, start + 12, 2, 1),
        field("sensor-index", SENSOR_INDEX, start + 14, 1, 1),
    ]
}

pub const fn field(
    name: &'static str,
    parameter: usize,
    output: u32,
    width: u8,
    count: u32,
) -> OutputField {
    OutputField {
        name,
        parameter: parameter as u32,
        output,
        width,
        count,
    }
}
