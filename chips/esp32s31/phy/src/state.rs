//! Typed software state retained by the ESP32-S31 PHY.
//!
//! The vendor implementation stores unrelated configuration, calibration and
//! runtime values in a version-specific `phy_param` byte image. The Rust driver
//! keeps those values in semantic fields instead.  Vendor offsets belong to
//! vendor-comparison code, never to the live radio owner.

#[cfg(test)]
use crate::calibration::bluetooth::{PhyBluetoothTxDcTransition, PhyBluetoothTxPowerTransition};
use crate::rx::gain_calibration::FINE_CODES;
use crate::{
    analog::crystal_duty::XtalDutyCalibrationParameters,
    analog::dcode::{PhyDcodeOutcome, PhyDcodeParameters},
    analog::frequency::PhyChannelFrequencyInitControl,
    analog::i2c::{FilterDcapParameters, PhyRfInitParameterSnapshot, PhyRfInitPrefixOutcome},
    analog::pbus::memory::PhyPbusMemoryParameters,
    analog::temperature::PhyTemperatureOutcome,
    calibration::baseband::{
        PHY_RX_TABLE_ENTRY_COUNT, PHY_SHARED_RX_GAIN_LAST_INDEX, PHY_WIFI_RX_GAIN_LAST_INDEX,
        PhyGeneratedRxGainTable, PhyRegisterInitParameters, PhyRxGainMemoryParameters,
        PhyRxTableInitParameters,
    },
    calibration::bluetooth::{
        PhyBluetoothTxDcPwdetTransition, PhyBluetoothTxGainImage, PhyBluetoothTxGainInitOutcome,
        PhyBluetoothTxGainInitParameters, PhyBluetoothTxGainInitTransition,
        PhyBluetoothTxGainParameters, PhyBluetoothTxPowerOutcome, PhyBluetoothTxPowerParameters,
        calculate_bluetooth_tx_gain,
    },
    channel::{
        PhyChipChannelOutcome, PhyChipChannelParameters, PhyWifiTxGainImage, PhyWifiTxGainRequest,
        calculate_wifi_tx_gain,
    },
    rx::gain::{PhyRxGainInitOutcome, PhyRxGainInitParameters},
    rx::gain_calibration::{PhyRxGainDcOutcome, PhyRxGainDcParameters},
    rx::iq::{PhyRxIqAdjustedTxParameters, PhyRxIqInitOutcome, PhyRxIqInitParameters},
    rx::saturation::PhyRxSaturationOutcome,
    tracking::calibration::{PhyCalibrationTrackingOutcome, PhyCalibrationTrackingParameters},
    tracking::i2c::{
        PhyWifiI2cTrackingBand, PhyWifiI2cTrackingOutcome, PhyWifiI2cTrackingParameters,
    },
    tracking::power::{PhyTxPowerTrackingOutcome, PhyTxPowerTrackingParameters},
    tx::calibration::{PhyTxCalibrationParameters, PhyTxCapOutcome, PhyTxCapParameters},
    tx::dc_offset::{PhyTxDcOutcome, PhyTxDcParameters},
    tx::dc_power_detector::{PhyTxDcPwdetOutcome, PhyTxDcPwdetParameters},
    tx::iq::{PhyTxIqInitOutcome, PhyTxIqInitParameters},
    tx::power::{
        PHY_TX_TARGET_POWER_COUNT, PhyTxPowerOutcome, PhyTxPowerParameters, PhyTxTargetPowerProfile,
    },
    tx::power_detector::{PhyPwdetOutcome, PhyPwdetParameters},
};

const fn saturate_phy_value(value: i32, upper: u8, lower: u8) -> u8 {
    if value < lower as i32 {
        lower
    } else if value > upper as i32 {
        upper
    } else {
        value as u8
    }
}

/// Immutable parameters selected for one ESP32-S31 radio instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyConfig {
    pbus_rx_path: u8,
    tone_selector: u8,
    tx_power_path: u8,
    bluetooth_power_path: u8,
    bluetooth_tx_path: u8,
    power_offset: i16,
    initial_calibration_attenuation: u8,
    bluetooth_tx_gain_attenuation: u8,
    initial_wifi_tx_gain_base: u8,
    initial_bluetooth_tx_gain_base: u8,
    target_power_maximum: i8,
    target_power: [i8; PHY_TX_TARGET_POWER_COUNT],
    target_adjustment: u8,
    channel_14_mic_enabled: bool,
    tx_gain_skip_publication: bool,
}

impl PhyConfig {
    /// Initial source-owned S31 configuration before board policy clamps TX.
    pub const fn esp32s31_default() -> Self {
        Self {
            pbus_rx_path: 0xbf,
            tone_selector: 0x20,
            tx_power_path: 0x1f,
            bluetooth_power_path: 0x16,
            bluetooth_tx_path: 1,
            power_offset: 0x160,
            initial_calibration_attenuation: 0x50,
            bluetooth_tx_gain_attenuation: 0,
            initial_wifi_tx_gain_base: 0,
            initial_bluetooth_tx_gain_base: 0,
            target_power_maximum: 0x54,
            target_power: [0; PHY_TX_TARGET_POWER_COUNT],
            target_adjustment: 0,
            channel_14_mic_enabled: false,
            tx_gain_skip_publication: false,
        }
    }

    /// Qualified production defaults that the vendor init selects from
    /// profile. Unused bytes from that profile are deliberately absent.
    pub const fn production() -> Self {
        Self::esp32s31_default().with_target_power(
            0x54,
            [
                0x50, 0x50, 0x50, 0x50, 0x4c, 0x48, 0x50, 0x50, 0x4c, 0x48, 0x40, 0x3c, 0x3c, 0x3c,
                0x4c, 0x4c, 0x48, 0x44,
            ],
        )
    }

    /// The largest calibrated target power over every rate, in quarter dBm:
    /// the most the PHY tunes any transmission to.
    pub const fn highest_target_power_quarter_dbm(&self) -> i8 {
        let mut highest = self.target_power[0];
        let mut index = 1;
        while index < PHY_TX_TARGET_POWER_COUNT {
            if self.target_power[index] > highest {
                highest = self.target_power[index];
            }
            index += 1;
        }
        highest
    }

    pub const fn with_target_power(
        mut self,
        maximum: i8,
        target: [i8; PHY_TX_TARGET_POWER_COUNT],
    ) -> Self {
        self.target_power_maximum = maximum;
        self.target_power = target;
        self
    }
}

impl Default for PhyConfig {
    fn default() -> Self {
        Self::esp32s31_default()
    }
}

/// The tracking reference temperatures of one registration, in °C.
///
/// Each reference changes only at its update point below, which is where
/// the vendor stores the matching `phy_param` byte; the offsets appear only
/// in the calibration cross-check, not here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TemperatureReferences {
    /// RFPLL capacitor tracking (`phy_param[130]`).
    rfpll: i16,
    /// Common RX calibration tracking: DCODE, RX gain tables and the channel
    /// refresh (`phy_param[400]`).
    common: i16,
    /// TX DC and power-detector tracking (`phy_param[72]`).
    transmit: i16,
    /// TX-power reference, the registration temperature before the TX-power
    /// calibration (`phy_param[302]`).
    power: i16,
    /// Temperature of the last TX-power tracking (`phy_param[4]`).
    power_tracking: i16,
}

impl TemperatureReferences {
    const ZERO: Self = Self {
        rfpll: 0,
        common: 0,
        transmit: 0,
        power: 0,
        power_tracking: 0,
    };

    pub(crate) const fn rfpll(&self) -> i16 {
        self.rfpll
    }

    pub(crate) const fn common(&self) -> i16 {
        self.common
    }

    pub(crate) const fn transmit(&self) -> i16 {
        self.transmit
    }

    pub(crate) const fn power(&self) -> i16 {
        self.power
    }

    pub(crate) const fn power_tracking(&self) -> i16 {
        self.power_tracking
    }

    /// The registration temperature step (`phy_get_temp_init`): before the
    /// frequency table exists it becomes the RFPLL reference, before the
    /// TX-power calibration the TX-power and transmit references, and every
    /// registration restarts TX-power tracking from the TX-power reference.
    fn commit_registration(&mut self, temperature: i16, control: PhyRegisterTemperatureControl) {
        if control.updates_offset_130() {
            self.rfpll = temperature;
        }
        if control.updates_reference_copies() {
            self.power = temperature;
            self.transmit = temperature;
        }
        self.restart_power_tracking();
    }

    /// Restart TX-power tracking from the TX-power reference.
    const fn restart_power_tracking(&mut self) {
        self.power_tracking = self.power;
    }

    /// Freshly generated RX gain tables (`phy_set_rx_gain_table`).
    const fn commit_rx_gain_tables(&mut self, temperature: i16) {
        self.common = temperature;
    }

    /// An RFPLL tracking run that applied a correction.
    const fn commit_rfpll(&mut self, temperature: i16) {
        self.rfpll = temperature;
    }

    /// A calibration tracking run whose common branch completed.
    const fn commit_common(&mut self, temperature: i16) {
        self.common = temperature;
    }

    /// A calibration tracking run whose transmit branch completed.
    const fn commit_transmit(&mut self, temperature: i16) {
        self.transmit = temperature;
    }

    /// A TX-power tracking run that updated the gain.
    const fn commit_power_tracking(&mut self, temperature: i16) {
        self.power_tracking = temperature;
    }

    /// The references a retained calibration restores; TX-power tracking
    /// restarts at the next registration.
    const fn restored(rfpll: i16, common: i16, transmit: i16, power: i16) -> Self {
        Self {
            rfpll,
            common,
            transmit,
            power,
            power_tracking: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommonPhyState {
    temperature: i16,
    temperature_acquisition: crate::tracking::temperature::StoredAcquisition,
    references: TemperatureReferences,
    tracking_gain_base: i8,
    calibrated_attenuation: u8,
    sensor_index: u8,
    crystal_selector: u8,
    rc_result: u8,
    filter_dcap: [u8; 5],
    rc_calibrated: bool,
    dcode: [u8; 8],
    i2c_frequency_parameter: u8,
    xtal_duty: [u8; 3],
    frequency_table_initialized: bool,
    front_end_parameter: bool,
    clear_tone_after_ready: bool,
    initialization_parameter: bool,
    registered: bool,
    temperature_debug: [u8; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WifiCalibrationStatus(u8);

impl WifiCalibrationStatus {
    const BASEBAND: u8 = 1 << 0;
    const PWDET: u8 = 1 << 1;
    const TX_POWER: u8 = 1 << 2;
    const TX_IQ: u8 = 1 << 3;
    const RX_GAIN_DC: u8 = 1 << 4;
    const RX_GAIN_TABLES: u8 = 1 << 5;
    const RX_SATURATION: u8 = 1 << 6;

    const EMPTY: Self = Self(0);

    const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WifiPhyState {
    // Current archive's independent additive gain input. Its initial value is
    // zero; no full-calibration producer or physical unit is established yet.
    tx_gain_adjustment: i8,
    tracking_gain_base: u8,
    dot11p_enabled: u8,
    dot11p_configuration: u8,
    current_level: u8,
    tx_power_tracking_slow: u8,
    tx_i2c_tracking_band: PhyWifiI2cTrackingBand,
    calibration: WifiCalibrationStatus,
    tx_dco: [[u16; 4]; 5],
    tx_reference_codes: [i16; 2],
    tx_capacitance: [u8; 6],
    tx_power_curve: [i8; 3],
    tx_power_corrections: [i8; 3],
    tx_power_adjustment: i8,
    tx_iq_config: u16,
    tx_iq_coefficient: u16,
    rx_iq_coefficients: [u16; 4],
    external_dcode: [u8; 2],
    calibration_temperature: i16,
    current_channel: u16,
    channel_initialized: bool,
    channel_bandwidth: u8,
    wifi_rx_table_last_index: u8,
    shared_rx_table_last_index: u8,
    wifi_index_dc: [[u16; 2]; 8],
    wifi_fine_dc: [[u16; 2]; FINE_CODES],
    shared_index_dc: [[u16; 2]; 11],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BluetoothPhyState {
    power_tracking: u8,
    tracking_gain_base: u8,
    tx_dc_calibrated: bool,
    tx_power_calibrated: bool,
    tx_dco: [[u16; 4]; 3],
    tx_power_curve: [i8; 3],
    tx_power_corrections: [i8; 3],
    tx_power_adjustment: i8,
    channel_base: u8,
}

/// Unique owner of all software state required after PHY registration.
///
/// The type is intentionally neither `Copy` nor `Clone`: moving it transfers
/// the authority to update the calibrated radio state.
pub struct PhyState {
    config: PhyConfig,
    common: CommonPhyState,
    wifi: WifiPhyState,
    bluetooth: BluetoothPhyState,
}

/// Typed retained calibration for one physical ESP32-S31 radio.
///
/// This is deliberately not `Copy` or `Clone`: persistence code must move the
/// one cache value instead of duplicating a vendor memory image.
pub struct PhyCalibrationCache {
    snapshot: PhyCalibrationSnapshot,
}

/// Stable, value-only boundary used by caller-selected persistence backends.
///
/// This is not a memory image of vendor state. It captures semantic results
/// from the calibration consumers. Cold registration validates these values,
/// restores the calibration state, and republishes hardware state which does
/// not survive a cold epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCalibrationSnapshot {
    pub schema: u16,
    pub identity: crate::calibration::registration::PhyCalibrationIdentity,
    pub common: PhyCommonCalibration,
    pub wifi: PhyWifiCalibration,
    pub bluetooth: PhyBluetoothCalibration,
}

pub const PHY_CALIBRATION_SNAPSHOT_SCHEMA: u16 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyCalibrationCacheError {
    SchemaMismatch,
    IdentityMismatch,
    IncompleteCalibration,
    InvalidRxGainTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyCommonCalibration {
    /// Most recently observed temperature. This is observation state, not a
    /// calibration reference for any runtime tracking branch.
    pub temperature: i16,
    pub rfpll_reference_temperature: i16,
    pub rxcal_reference_temperature: i16,
    pub txcal_reference_temperature: i16,
    /// `phy_param[302]`, which the vendor's retained calibration restores and
    /// every registration copies into the tracking temperature.
    pub power_reference_temperature: i16,
    pub sensor_index: u8,
    pub crystal_selector: u8,
    pub rc_result: u8,
    pub filter_dcap: [u8; 5],
    pub rc_calibrated: bool,
    pub dcode: [u8; 8],
    pub i2c_frequency_parameter: u8,
    pub xtal_duty: [u8; 3],
    pub clear_tone_after_ready: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiCalibration {
    /// Independent additive gain input; does not encode Bluetooth attenuation.
    pub tx_gain_adjustment: i8,
    pub baseband_calibrated: bool,
    pub pwdet_calibrated: bool,
    pub tx_power_calibrated: bool,
    pub tx_iq_calibrated: bool,
    pub rx_gain_dc_calibrated: bool,
    pub rx_gain_tables_initialized: bool,
    pub rx_saturation_detected: bool,
    pub tx_dco: [[u16; 4]; 5],
    pub tx_reference_codes: [i16; 2],
    pub tx_capacitance: [u8; 6],
    pub tx_power_curve: [i8; 3],
    pub tx_power_corrections: [i8; 3],
    pub tx_power_adjustment: i8,
    pub calibrated_attenuation: u8,
    pub tx_iq_config: u16,
    pub tx_iq_coefficient: u16,
    pub rx_iq_coefficients: [u16; 4],
    pub external_dcode: [u8; 2],
    pub calibration_temperature: i16,
    pub calibration_channel: u16,
    pub wifi_rx_table_last_index: u8,
    pub shared_rx_table_last_index: u8,
    pub wifi_index_dc: [[u16; 2]; 8],
    pub wifi_fine_dc: [[u16; 2]; FINE_CODES],
    pub shared_index_dc: [[u16; 2]; 11],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyBluetoothCalibration {
    pub tx_dc_calibrated: bool,
    pub tx_power_calibrated: bool,
    pub tx_dco: [[u16; 4]; 3],
    pub tx_power_curve: [i8; 3],
    pub tx_power_corrections: [i8; 3],
    pub tx_power_adjustment: i8,
}

impl PhyCalibrationCache {
    pub(crate) const fn capture(
        identity: crate::calibration::registration::PhyCalibrationIdentity,
        state: &PhyState,
    ) -> Self {
        Self {
            snapshot: state.calibration_snapshot(identity),
        }
    }

    pub const fn from_snapshot(snapshot: PhyCalibrationSnapshot) -> Option<Self> {
        if snapshot.schema == PHY_CALIBRATION_SNAPSHOT_SCHEMA {
            Some(Self { snapshot })
        } else {
            None
        }
    }

    pub const fn snapshot(&self) -> &PhyCalibrationSnapshot {
        &self.snapshot
    }

    pub const fn into_snapshot(self) -> PhyCalibrationSnapshot {
        self.snapshot
    }

    pub const fn identity(&self) -> crate::calibration::registration::PhyCalibrationIdentity {
        self.snapshot.identity
    }

    pub fn matches(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> bool {
        self.snapshot.schema == PHY_CALIBRATION_SNAPSHOT_SCHEMA
            && self.snapshot.identity == identity
    }

    /// Validate that this snapshot can seed a cold partial-calibration run.
    ///
    /// Identity and completion guards are checked before any live state is
    /// changed. Hardware-resident frequency and RX gain tables are deliberately
    /// not trusted as retained; registration regenerates and republishes them.
    pub fn validate_for_replay(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> Result<(), PhyCalibrationCacheError> {
        let snapshot = &self.snapshot;
        if snapshot.schema != PHY_CALIBRATION_SNAPSHOT_SCHEMA {
            return Err(PhyCalibrationCacheError::SchemaMismatch);
        }
        if snapshot.identity != identity {
            return Err(PhyCalibrationCacheError::IdentityMismatch);
        }
        if !snapshot.common.rc_calibrated
            || !snapshot.wifi.baseband_calibrated
            || !snapshot.wifi.pwdet_calibrated
            || !snapshot.wifi.tx_power_calibrated
            || !snapshot.wifi.tx_iq_calibrated
            || !snapshot.wifi.rx_gain_dc_calibrated
            || !snapshot.wifi.rx_gain_tables_initialized
            || !snapshot.bluetooth.tx_dc_calibrated
            || !snapshot.bluetooth.tx_power_calibrated
        {
            return Err(PhyCalibrationCacheError::IncompleteCalibration);
        }
        if snapshot.wifi.wifi_rx_table_last_index != PHY_WIFI_RX_GAIN_LAST_INDEX
            || snapshot.wifi.shared_rx_table_last_index != PHY_SHARED_RX_GAIN_LAST_INDEX
        {
            return Err(PhyCalibrationCacheError::InvalidRxGainTable);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyDot11pConfiguration {
    pub enabled: u8,
    pub configuration: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyTemperatureTrackingDebug {
    pub first: u8,
    pub second: u8,
}

impl PhyTemperatureTrackingDebug {
    /// Optional RFPLL-cap threshold selected by the debug policy's first flag.
    pub const fn rfpll_threshold_override(self) -> Option<u8> {
        if self.first & 1 != 0 {
            Some(self.second)
        } else {
            None
        }
    }

    /// Optional calibration threshold selected independently by the second
    /// debug-policy flag. Both children intentionally share the same value.
    pub const fn calibration_threshold_override(self) -> Option<u8> {
        if self.first & 2 != 0 {
            Some(self.second)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyRegisterTemperatureControl {
    update_registration_reference: bool,
    update_calibration_reference: bool,
}

impl PhyRegisterTemperatureControl {
    pub const FULL: Self = Self {
        update_registration_reference: true,
        update_calibration_reference: true,
    };

    pub const fn updates_offset_130(self) -> bool {
        self.update_registration_reference
    }

    pub const fn updates_reference_copies(self) -> bool {
        self.update_calibration_reference
    }
}

impl PhyState {
    pub const fn new(config: PhyConfig) -> Self {
        let calibrated_attenuation = config.initial_calibration_attenuation;
        let wifi_tracking_gain_base = config.initial_wifi_tx_gain_base;
        let bluetooth_tracking_gain_base = config.initial_bluetooth_tx_gain_base;
        Self {
            config,
            common: CommonPhyState {
                temperature: 0,
                temperature_acquisition:
                    crate::tracking::temperature::StoredAcquisition::UNOBSERVED,
                references: TemperatureReferences::ZERO,
                tracking_gain_base: 0,
                calibrated_attenuation,
                sensor_index: 2,
                crystal_selector: 0,
                rc_result: 0,
                filter_dcap: [0; 5],
                rc_calibrated: false,
                dcode: [0; 8],
                i2c_frequency_parameter: 0,
                xtal_duty: [0; 3],
                frequency_table_initialized: false,
                front_end_parameter: true,
                clear_tone_after_ready: false,
                initialization_parameter: false,
                registered: false,
                temperature_debug: [0; 2],
            },
            wifi: WifiPhyState {
                tx_gain_adjustment: 0,
                tracking_gain_base: wifi_tracking_gain_base,
                dot11p_enabled: 0,
                dot11p_configuration: 0,
                current_level: 0,
                tx_power_tracking_slow: 1,
                tx_i2c_tracking_band: PhyWifiI2cTrackingBand::Nominal,
                calibration: WifiCalibrationStatus::EMPTY,
                tx_dco: [[0; 4]; 5],
                tx_reference_codes: [0; 2],
                tx_capacitance: [0; 6],
                tx_power_curve: [0; 3],
                tx_power_corrections: [0; 3],
                tx_power_adjustment: 0,
                tx_iq_config: 0,
                tx_iq_coefficient: 0,
                rx_iq_coefficients: [0; 4],
                external_dcode: [0; 2],
                calibration_temperature: 0,
                current_channel: 0,
                channel_initialized: false,
                channel_bandwidth: 0,
                wifi_rx_table_last_index: 0,
                shared_rx_table_last_index: 0,
                wifi_index_dc: [[0; 2]; 8],
                wifi_fine_dc: [[0; 2]; FINE_CODES],
                shared_index_dc: [[0; 2]; 11],
            },
            bluetooth: BluetoothPhyState {
                power_tracking: 1,
                tracking_gain_base: bluetooth_tracking_gain_base,
                tx_dc_calibrated: false,
                tx_power_calibrated: false,
                tx_dco: [[0; 4]; 3],
                tx_power_curve: [0; 3],
                tx_power_corrections: [0; 3],
                tx_power_adjustment: 0,
                channel_base: 0,
            },
        }
    }

    /// A probe image's semantic fixture (`cfg(oer_probe)`): it mints no
    /// registered epoch or RF access.
    #[cfg(oer_probe)]
    pub(crate) fn calibration_tracking_fixture(
        parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
    ) -> Self {
        let mut state = Self::default();
        state.common.temperature = parameters.current_temperature;
        state.common.references.common = parameters.common_reference_temperature;
        state.common.references.transmit = parameters.transmit_reference_temperature;
        state.common.crystal_selector = parameters.crystal_selector;
        state.wifi.current_channel = parameters.current_channel;
        state.wifi.channel_bandwidth = parameters.channel_bandwidth;
        if let Some(threshold) = parameters.threshold_override {
            state.common.temperature_debug = [2, threshold];
        }
        state
    }

    /// [`Self::calibration_tracking_fixture`] with the parameter parent's
    /// retained inputs (`cfg(oer_probe)`).
    #[cfg(oer_probe)]
    pub(crate) fn parameter_tracking_fixture(
        parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
        gain_adjustment: i8,
        relaxed_threshold: bool,
    ) -> Self {
        let mut state = Self::calibration_tracking_fixture(parameters);
        state.wifi.tx_gain_adjustment = gain_adjustment;
        state.wifi.tx_power_tracking_slow = relaxed_threshold.into();
        state
    }

    /// Seed the retained RX-gain table last indices as the vendor comparison
    /// seeds its own state (`cfg(oer_probe)`).
    #[cfg(oer_probe)]
    pub(crate) fn seed_rx_table_last_indices(&mut self, shared: u8, wifi: u8) {
        self.wifi.shared_rx_table_last_index = shared;
        self.wifi.wifi_rx_table_last_index = wifi;
    }

    pub const fn config(&self) -> &PhyConfig {
        &self.config
    }

    #[cfg(any(test, oer_probe))]
    pub fn set_tx_power_tracking_slow(&mut self, value: u8) {
        self.wifi.tx_power_tracking_slow = value;
    }

    pub const fn tx_power_tracking_slow(&self) -> u8 {
        self.wifi.tx_power_tracking_slow
    }

    pub const fn wifi_i2c_tracking_parameters(&self) -> PhyWifiI2cTrackingParameters {
        PhyWifiI2cTrackingParameters {
            current_temperature: self.common.temperature,
            previous_band: self.wifi.tx_i2c_tracking_band,
        }
    }

    pub fn apply_wifi_i2c_tracking_outcome(&mut self, outcome: PhyWifiI2cTrackingOutcome) {
        self.wifi.tx_i2c_tracking_band = outcome.band;
    }

    /// Owned 802.11p configuration. Channel selection has no 802.11p step:
    /// complete `phy_chip_set_chan` passes these bytes to `phy_11p_set`,
    /// whose body only writes them back to the same `phy_param` bytes.
    #[cfg(any(test, oer_probe))]
    pub fn set_dot11p_configuration(&mut self, enabled: u8, configuration: u8) {
        self.wifi.dot11p_enabled = enabled;
        self.wifi.dot11p_configuration = configuration;
    }

    pub const fn dot11p_configuration(&self) -> PhyDot11pConfiguration {
        PhyDot11pConfiguration {
            enabled: self.wifi.dot11p_enabled,
            configuration: self.wifi.dot11p_configuration,
        }
    }

    #[cfg(any(test, oer_probe))]
    pub fn set_current_level(&mut self, value: u8) {
        self.wifi.current_level = value;
    }

    pub const fn current_level(&self) -> u8 {
        self.wifi.current_level
    }

    #[cfg(any(test, oer_probe))]
    pub fn set_bt_power_tracking(&mut self, value: u8) {
        self.bluetooth.power_tracking = value;
    }

    pub const fn bt_power_tracking(&self) -> u8 {
        self.bluetooth.power_tracking
    }

    #[cfg(test)]
    pub(crate) const fn bluetooth_tx_dc_transition(&self) -> PhyBluetoothTxDcTransition {
        PhyBluetoothTxDcTransition::new(
            PhyTxDcParameters {
                pbus_rx_path_value: self.config.pbus_rx_path,
            },
            self.config.bluetooth_tx_path,
        )
    }

    pub fn apply_bluetooth_tx_dc_outcome(&mut self, outcome: PhyTxDcOutcome) {
        let mut row = 0;
        while row != self.bluetooth.tx_dco.len() {
            self.bluetooth.tx_dco[row] = outcome.dco[row];
            row += 1;
        }
        self.bluetooth.tx_dc_calibrated = true;
    }

    pub const fn bluetooth_tx_dc_calibrated(&self) -> bool {
        self.bluetooth.tx_dc_calibrated
    }

    #[cfg(test)]
    pub(crate) const fn bluetooth_tx_dco(&self) -> [[u16; 4]; 3] {
        self.bluetooth.tx_dco
    }

    pub(crate) const fn bluetooth_tx_dc_pwdet_transition(&self) -> PhyBluetoothTxDcPwdetTransition {
        PhyBluetoothTxDcPwdetTransition::new(
            PhyTxDcPwdetParameters {
                dco: self.bluetooth.tx_dco,
                clear_tone_after_ready: self.common.clear_tone_after_ready,
            },
            self.config.bluetooth_tx_path,
        )
    }

    pub fn apply_bluetooth_tx_dc_pwdet_outcome(&mut self, outcome: PhyTxDcPwdetOutcome) {
        self.bluetooth.tx_dco = outcome.dco;
    }

    pub fn bluetooth_tx_power_parameters(&self) -> PhyBluetoothTxPowerParameters {
        let mut calibration = self.tx_power_parameters();
        calibration.already_calibrated = self.bluetooth.tx_power_calibrated;
        PhyBluetoothTxPowerParameters {
            calibration,
            pbus_power_path_value: self.config.bluetooth_power_path,
            pbus_tx_path_value: self.config.bluetooth_tx_path,
            dco: self.bluetooth.tx_dco[0],
            tone_selector: self.config.tone_selector as u16,
        }
    }

    #[cfg(test)]
    pub(crate) fn bluetooth_tx_power_transition(&self) -> PhyBluetoothTxPowerTransition {
        PhyBluetoothTxPowerTransition::new(self.bluetooth_tx_power_parameters())
    }

    pub fn apply_bluetooth_tx_power_outcome(&mut self, outcome: PhyBluetoothTxPowerOutcome) {
        let calibration = outcome.calibration;
        if !calibration.calibration_performed {
            return;
        }
        self.bluetooth.tx_power_corrections = calibration.point_corrections;
        self.bluetooth.tx_power_curve = calibration.power_curve;
        self.bluetooth.tx_power_adjustment = calibration.power_adjustment;
        self.common.calibrated_attenuation = calibration.final_attenuation;
        self.common.clear_tone_after_ready = false;
        self.bluetooth.tx_power_calibrated = true;
    }

    pub const fn bluetooth_tx_power_calibrated(&self) -> bool {
        self.bluetooth.tx_power_calibrated
    }

    #[cfg(test)]
    pub(crate) const fn bluetooth_tx_power_result(&self) -> ([i8; 3], [i8; 3], i8) {
        (
            self.bluetooth.tx_power_curve,
            self.bluetooth.tx_power_corrections,
            self.bluetooth.tx_power_adjustment,
        )
    }

    const fn packed_seed(rows: &[[u16; 4]; 3]) -> [u32; 6] {
        let mut seed = [0; 6];
        let mut index = 0;
        while index != seed.len() {
            let first = rows[index / 2][(index % 2) * 2];
            let second = rows[index / 2][(index % 2) * 2 + 1];
            seed[index] = first as u32 | ((second as u32) << 16);
            index += 1;
        }
        seed
    }

    pub const fn bluetooth_tx_gain_parameters(&self) -> PhyBluetoothTxGainParameters {
        PhyBluetoothTxGainParameters {
            seed: Self::packed_seed(&self.bluetooth.tx_dco),
            config: self.wifi.tx_iq_config,
            calibration_curve: [
                self.bluetooth.tx_power_curve[0] as u8,
                self.bluetooth.tx_power_curve[1] as u8,
                self.bluetooth.tx_power_curve[2] as u8,
            ],
            correction: self.bluetooth.tx_power_adjustment,
            base: self.bluetooth.tracking_gain_base,
            attenuation: self.config.bluetooth_tx_gain_attenuation,
        }
    }

    pub const fn bluetooth_tx_gain_image(&self) -> PhyBluetoothTxGainImage {
        calculate_bluetooth_tx_gain(self.bluetooth_tx_gain_parameters())
    }

    /// Regenerate the shared Bluetooth/IEEE 802.15.4 gain bank from the live
    /// calibration state and one temperature-tracked base value.
    pub const fn bluetooth_ieee802154_tracking_gain_image(
        &self,
        gain_base: i8,
    ) -> PhyBluetoothTxGainImage {
        let mut parameters = self.bluetooth_tx_gain_parameters();
        parameters.base = gain_base as u8;
        calculate_bluetooth_tx_gain(parameters)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn tracking_gain_generator(
        &self,
        class: crate::tracking::parameters::PhyCalibrationTrackClass,
        channel: u16,
        gain_base: i8,
    ) -> Option<crate::tx::gain::Generator> {
        use crate::tracking::parameters::PhyCalibrationTrackClass as Class;
        match class {
            Class::Wifi => {
                let parameters = self.channel_parameters();
                if parameters.tx_gain_skip_publication {
                    return None;
                }
                Some(crate::channel::begin_wifi_tx_gain(PhyWifiTxGainRequest {
                    channel,
                    calibration_curve: parameters.tx_gain_curve,
                    correction: parameters.tx_gain_correction,
                    base_and_delta: (gain_base as u8)
                        .wrapping_add(parameters.tx_gain_adjustment as u8)
                        as i8,
                }))
            }
            Class::BluetoothIeee802154 => {
                let mut parameters = self.bluetooth_tx_gain_parameters();
                parameters.base = gain_base as u8;
                Some(crate::calibration::bluetooth::begin_bluetooth_tx_gain(
                    parameters,
                ))
            }
        }
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn tracking_gain_seed(
        &self,
        class: crate::tracking::parameters::PhyCalibrationTrackClass,
    ) -> ([u32; 6], u16) {
        let seed = match class {
            crate::tracking::parameters::PhyCalibrationTrackClass::Wifi => self.wifi_seed(),
            crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154 => {
                Self::packed_seed(&self.bluetooth.tx_dco)
            }
        };
        (seed, self.wifi.tx_iq_config)
    }

    /// Regenerate the shared calibration gain bank from a just-completed
    /// TXDC/PWDET result that has not yet reached the outer state commit.
    pub const fn bluetooth_ieee802154_calibration_gain_image(
        &self,
        tx_dc_pwdet: PhyTxDcPwdetOutcome,
    ) -> PhyBluetoothTxGainImage {
        let mut parameters = self.bluetooth_tx_gain_parameters();
        parameters.seed = Self::packed_seed(&tx_dc_pwdet.dco);
        calculate_bluetooth_tx_gain(parameters)
    }

    pub(crate) fn bluetooth_tx_gain_init_transition(
        &self,
        depth: crate::analog::pbus::PhyForceTxRxDepth,
    ) -> PhyBluetoothTxGainInitTransition {
        PhyBluetoothTxGainInitTransition::new(
            PhyBluetoothTxGainInitParameters {
                crystal_selector: self.common.crystal_selector,
                capacitance: self.wifi.tx_capacitance,
                tx_dc_calibrated: self.bluetooth.tx_dc_calibrated,
                tx_dc: PhyTxDcParameters {
                    pbus_rx_path_value: self.config.pbus_rx_path,
                },
                tx_path_value: self.config.bluetooth_tx_path,
                tx_power: self.bluetooth_tx_power_parameters(),
                tx_dc_pwdet: PhyTxDcPwdetParameters {
                    dco: self.bluetooth.tx_dco,
                    clear_tone_after_ready: self.common.clear_tone_after_ready,
                },
                gain: self.bluetooth_tx_gain_parameters(),
            },
            depth,
        )
    }

    pub fn apply_bluetooth_tx_gain_init_outcome(&mut self, outcome: PhyBluetoothTxGainInitOutcome) {
        self.bluetooth.tx_dc_calibrated = outcome.tx_dc_calibrated;
        self.bluetooth.tx_dco = outcome.dco;
        self.apply_bluetooth_tx_power_outcome(outcome.tx_power);
    }

    #[cfg(any(test, oer_probe))]
    pub fn set_ble_channel_base(&mut self, value: u8) {
        self.bluetooth.channel_base = value;
    }

    #[cfg(any(test, oer_probe))]
    pub const fn ble_channel_base(&self) -> u8 {
        self.bluetooth.channel_base
    }

    #[cfg(any(test, oer_probe))]
    pub fn set_initialization_parameter(&mut self, value: u32) {
        self.common.initialization_parameter = value & 1 != 0;
    }

    pub const fn initialization_parameter(&self) -> bool {
        self.common.initialization_parameter
    }

    pub fn set_temperature_tracking_debug(&mut self, first: u8, second: u8) {
        self.common.temperature_debug = [first, second];
    }

    pub const fn temperature_tracking_debug(&self) -> PhyTemperatureTrackingDebug {
        PhyTemperatureTrackingDebug {
            first: self.common.temperature_debug[0],
            second: self.common.temperature_debug[1],
        }
    }

    pub const fn tx_target_power_profile(&self) -> PhyTxTargetPowerProfile {
        PhyTxTargetPowerProfile::new(self.config.target_power_maximum, self.config.target_power)
    }

    pub const fn baseband_calibration_complete(&self) -> bool {
        self.wifi
            .calibration
            .contains(WifiCalibrationStatus::BASEBAND)
    }

    pub fn mark_baseband_calibration_complete(&mut self) {
        self.wifi
            .calibration
            .set(WifiCalibrationStatus::BASEBAND, true);
    }

    pub const fn disable_wifi_after_baseband_init(&self) -> bool {
        self.common.initialization_parameter
    }

    pub const fn register_init_parameters(&self) -> PhyRegisterInitParameters {
        PhyRegisterInitParameters {
            parameter_121: self.wifi.wifi_rx_table_last_index,
            parameter_120: self.wifi.shared_rx_table_last_index,
        }
    }

    pub fn prepare_rx_table_init(&mut self) -> PhyRxTableInitParameters {
        self.wifi.wifi_rx_table_last_index = PHY_RX_TABLE_ENTRY_COUNT;
        self.wifi.shared_rx_table_last_index = PHY_RX_TABLE_ENTRY_COUNT;
        PhyRxTableInitParameters {
            parameter_002: self.config.pbus_rx_path,
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
        }
    }

    pub const fn rx_gain_memory_parameters(&self) -> PhyRxGainMemoryParameters {
        PhyRxGainMemoryParameters {
            parameter_002: self.config.pbus_rx_path,
            wifi_index_dc: self.wifi.wifi_index_dc,
            wifi_fine_dc: self.wifi.wifi_fine_dc,
            shared_index_dc: self.wifi.shared_index_dc,
            wifi_auxiliary: self.wifi.rx_iq_coefficients[0],
        }
    }

    pub const fn rx_gain_dc_parameters(&self) -> PhyRxGainDcParameters {
        PhyRxGainDcParameters {
            crystal_selector: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
            rx_saturation_detected: self
                .wifi
                .calibration
                .contains(WifiCalibrationStatus::RX_SATURATION),
        }
    }

    pub fn apply_rx_gain_dc_outcome(&mut self, outcome: PhyRxGainDcOutcome) {
        self.wifi.wifi_index_dc = outcome.wifi_index_dc;
        self.wifi.wifi_fine_dc = outcome.wifi_fine_dc;
        self.wifi.shared_index_dc = outcome.shared_index_dc;
    }

    pub const fn rx_gain_init_parameters(&self) -> PhyRxGainInitParameters {
        PhyRxGainInitParameters {
            dc_calibrated: self
                .wifi
                .calibration
                .contains(WifiCalibrationStatus::RX_GAIN_DC),
            tables_initialized: self
                .wifi
                .calibration
                .contains(WifiCalibrationStatus::RX_GAIN_TABLES),
            dc: self.rx_gain_dc_parameters(),
            memory: self.rx_gain_memory_parameters(),
        }
    }

    /// Commit an RX-gain initialization. Generated tables also commit the
    /// latest temperature as the common calibration reference, as
    /// `phy_set_rx_gain_table` of the pinned libphy stores `phy_param[0]`
    /// into `phy_param[400]` after publishing them.
    pub fn apply_rx_gain_init_outcome(&mut self, outcome: PhyRxGainInitOutcome) {
        if let Some(dc) = outcome.dc {
            self.apply_rx_gain_dc_outcome(dc);
            self.wifi
                .calibration
                .set(WifiCalibrationStatus::RX_GAIN_DC, true);
        }
        if outcome.generated_tables {
            self.wifi.wifi_rx_table_last_index = outcome.wifi_last_index.min(0x4f);
            self.wifi.shared_rx_table_last_index = outcome.shared_last_index.min(0x4f);
            self.wifi
                .calibration
                .set(WifiCalibrationStatus::RX_GAIN_TABLES, true);
            self.common
                .references
                .commit_rx_gain_tables(self.common.temperature);
        }
    }

    pub fn apply_generated_rx_gain_tables(
        &mut self,
        wifi: PhyGeneratedRxGainTable,
        shared: PhyGeneratedRxGainTable,
    ) {
        self.wifi.wifi_rx_table_last_index = wifi.last_index.min(PHY_RX_TABLE_ENTRY_COUNT);
        self.wifi.shared_rx_table_last_index = shared.last_index.min(PHY_RX_TABLE_ENTRY_COUNT);
        self.wifi
            .calibration
            .set(WifiCalibrationStatus::RX_GAIN_TABLES, true);
    }

    pub const fn rx_saturation_parameter_002(&self) -> u8 {
        self.config.pbus_rx_path
    }

    pub const fn pbus_memory_parameters(&self) -> PhyPbusMemoryParameters {
        PhyPbusMemoryParameters {
            parameter_002: self.config.pbus_rx_path,
            parameter_014: self.config.bluetooth_tx_path,
        }
    }

    pub fn apply_temperature_outcome(&mut self, outcome: PhyTemperatureOutcome) {
        self.common.temperature = outcome.temperature;
        self.common.temperature_acquisition = crate::tracking::temperature::StoredAcquisition::new(
            crate::tracking::temperature::Acquisition::Undated,
        );
        self.common.sensor_index = outcome.sensor_index;
    }

    /// Retained sensor value and its acquisition provenance, independent of
    /// the scheduler's last evaluation time.
    pub const fn temperature_observation(&self) -> crate::tracking::temperature::Observation {
        crate::tracking::temperature::Observation {
            value: self.common.temperature,
            acquisition: self.common.temperature_acquisition.get(),
        }
    }

    pub(crate) fn apply_observed_temperature_outcome(
        &mut self,
        outcome: PhyTemperatureOutcome,
        started: Option<u64>,
        completed: Option<u64>,
    ) {
        self.apply_temperature_outcome(outcome);
        if let (Some(started), Some(completed)) = (started, completed) {
            self.common.temperature_acquisition =
                crate::tracking::temperature::StoredAcquisition::new(
                    crate::tracking::temperature::Acquisition::Window { started, completed },
                );
        }
    }

    pub(crate) const fn rfpll_tracking_request(
        &self,
        threshold_override: Option<u8>,
    ) -> crate::tracking::rfpll::thermal::Request {
        crate::tracking::rfpll::thermal::Request {
            current_temperature: self.common.temperature,
            reference_temperature: self.common.references.rfpll(),
            current_channel: self.wifi.current_channel,
            threshold_override,
        }
    }

    pub(crate) fn commit_rfpll_tracking(
        &mut self,
        outcome: crate::tracking::rfpll::thermal::Outcome,
    ) {
        if outcome.correction.is_some() {
            self.common
                .references
                .commit_rfpll(outcome.reference_temperature);
        }
    }

    /// Project the RX and shared TX semantic references consumed by
    /// `phy_cal_param_track`.
    pub const fn calibration_tracking_parameters(
        &self,
        threshold_override: Option<u8>,
    ) -> PhyCalibrationTrackingParameters {
        PhyCalibrationTrackingParameters {
            current_temperature: self.common.temperature,
            common_reference_temperature: self.common.references.common(),
            transmit_reference_temperature: self.common.references.transmit(),
            threshold_override,
            current_channel: self.wifi.current_channel,
            channel_bandwidth: self.wifi.channel_bandwidth,
            crystal_selector: self.common.crystal_selector,
        }
    }

    /// Commit only references whose complete calibration branch reached its
    /// terminal restore edge.
    pub(crate) fn apply_calibration_tracking_outcome(
        &mut self,
        outcome: PhyCalibrationTrackingOutcome,
    ) {
        let common = match (
            outcome.common_updated,
            outcome.dcode,
            outcome.rx_gain,
            outcome.channel,
        ) {
            (true, Some(dcode), Some(rx_gain), Some(channel)) => {
                // The periodic parent forcibly clears both vendor completion
                // guards. Its successful child must therefore contain fresh
                // RX-DC data and freshly generated tables. The channel child
                // also refreshes temperature before the vendor stores the
                // common tracking reference.
                if rx_gain.dc.is_none()
                    || !rx_gain.generated_tables
                    || channel.temperature.temperature != outcome.common_reference_temperature
                {
                    return;
                }
                Some((dcode, rx_gain, channel))
            }
            (false, None, None, None) => None,
            // A terminal common branch publishes all three values atomically.
            // Reject structurally inconsistent caller-created outcomes.
            _ => return,
        };
        // Validate the complete requested TX set before publishing any result.
        if outcome.wifi_tx_dc_pwdet.is_some()
            != (outcome.transmit_updated && outcome.clients.wifi())
            || outcome.bluetooth_ieee802154_tx_dc_pwdet.is_some()
                != (outcome.transmit_updated && outcome.clients.bluetooth_ieee802154())
        {
            return;
        }

        if let Some((dcode, rx_gain, channel)) = common {
            self.apply_dcode_outcome(dcode);
            self.apply_rx_gain_init_outcome(rx_gain);
            self.apply_channel_outcome(channel);
            self.common
                .references
                .commit_common(outcome.common_reference_temperature);
        }
        if let Some(result) = outcome.wifi_tx_dc_pwdet {
            self.apply_tx_dc_pwdet_outcome(result);
        }
        if let Some(result) = outcome.bluetooth_ieee802154_tx_dc_pwdet {
            self.apply_bluetooth_tx_dc_pwdet_outcome(result);
        }
        if outcome.transmit_updated {
            self.common
                .references
                .commit_transmit(outcome.transmit_reference_temperature);
        }
    }

    /// Project the semantic state consumed by periodic TX-power tracking.
    ///
    /// `relaxed_threshold` is supplied by the reviewed outer tracking policy;
    /// it is not persisted as an unexplained vendor parameter byte.
    pub const fn tx_power_tracking_parameters(
        &self,
        relaxed_threshold: bool,
    ) -> PhyTxPowerTrackingParameters {
        PhyTxPowerTrackingParameters {
            current_temperature: self.common.temperature,
            reference_temperature: self.common.references.power(),
            previous_tracking_temperature: self.common.references.power_tracking(),
            previous_tracking_gain_base: self.common.tracking_gain_base,
            wifi_gain_base: self.wifi.tracking_gain_base as i8,
            bluetooth_ieee802154_gain_base: self.bluetooth.tracking_gain_base as i8,
            relaxed_threshold,
            wifi_gain_publication_skipped: self.config.tx_gain_skip_publication,
        }
    }

    /// Force the next diagnostic power child to regenerate its real gain bank.
    #[cfg(all(oer_debug_control, target_arch = "riscv32"))]
    pub(crate) fn force_power_tracking_debug(&mut self, wifi: bool) {
        use crate::tracking::{
            parameters::PhyCalibrationTrackClass as Class,
            power::{PhyTxPowerTrackingRequest, decide_tx_power_tracking},
        };
        let class = if wifi {
            Class::Wifi
        } else {
            Class::BluetoothIeee802154
        };
        let gain = decide_tx_power_tracking(
            PhyTxPowerTrackingRequest {
                class,
                enabled: true,
                wifi_channel: self.current_wifi_channel(),
            },
            self.tx_power_tracking_parameters(self.tx_power_tracking_slow() != 0),
        )
        .gain_base;
        if wifi {
            self.wifi.tracking_gain_base = (gain as u8).wrapping_add(1);
        } else {
            self.bluetooth.tracking_gain_base = (gain as u8).wrapping_add(1);
        }
    }

    /// Commit a completed tracking transaction to the live radio owner.
    pub fn apply_tx_power_tracking_outcome(&mut self, outcome: PhyTxPowerTrackingOutcome) {
        if !outcome.gain_updated {
            return;
        }
        self.common
            .references
            .commit_power_tracking(outcome.tracking_temperature);
        self.common.tracking_gain_base = outcome.tracking_gain_base;
        match outcome.class {
            crate::tracking::parameters::PhyCalibrationTrackClass::Wifi => {
                self.wifi.tracking_gain_base = outcome.wifi_gain_base as u8;
            }
            crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154 => {
                self.bluetooth.tracking_gain_base = outcome.bluetooth_ieee802154_gain_base as u8;
            }
        }
    }

    pub const fn dcode_parameters(&self) -> PhyDcodeParameters {
        PhyDcodeParameters {
            crystal_selector: self.common.crystal_selector,
        }
    }

    pub fn apply_dcode_outcome(&mut self, outcome: PhyDcodeOutcome) {
        self.common.dcode = outcome.codes;
    }

    pub const fn pwdet_parameters(&self) -> PhyPwdetParameters {
        PhyPwdetParameters {
            already_calibrated: self.wifi.calibration.contains(WifiCalibrationStatus::PWDET),
            pbus_tx_path_value: self.config.tx_power_path,
            pbus_rx_path_value: self.config.pbus_rx_path,
            dco: self.wifi.tx_dco[0],
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
        }
    }

    pub fn apply_pwdet_outcome(&mut self, outcome: PhyPwdetOutcome) {
        self.wifi.tx_reference_codes = outcome.reference_codes;
        if outcome.calibrated {
            self.wifi
                .calibration
                .set(WifiCalibrationStatus::PWDET, true);
        }
    }

    pub const fn tx_dc_parameters(&self) -> PhyTxDcParameters {
        PhyTxDcParameters {
            pbus_rx_path_value: self.config.pbus_rx_path,
        }
    }

    pub fn apply_tx_dc_outcome(&mut self, outcome: PhyTxDcOutcome) {
        self.wifi.tx_dco = outcome.dco;
    }

    pub const fn tx_cap_parameters(&self) -> PhyTxCapParameters {
        PhyTxCapParameters {
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
            power_offset: self.config.power_offset,
            initial_attenuation: self.common.calibrated_attenuation,
        }
    }

    pub fn apply_tx_cap_outcome(&mut self, outcome: PhyTxCapOutcome) {
        self.wifi.tx_capacitance = outcome.capacitance;
        self.common.calibrated_attenuation = outcome.attenuation;
    }

    pub const fn tx_power_parameters(&self) -> PhyTxPowerParameters {
        PhyTxPowerParameters {
            already_calibrated: self
                .wifi
                .calibration
                .contains(WifiCalibrationStatus::TX_POWER),
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            capacitance: self.wifi.tx_capacitance,
            target_adjustment: self.config.target_adjustment,
            power_offset: self.config.power_offset,
            initial_attenuation: self.common.calibrated_attenuation,
            clear_tone_after_ready: self.common.clear_tone_after_ready,
            reference_codes: self.wifi.tx_reference_codes,
        }
    }

    pub fn apply_tx_power_outcome(&mut self, outcome: PhyTxPowerOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        self.wifi.tx_reference_codes = outcome.reference_codes;
        self.wifi.tx_power_curve = outcome.power_curve;
        self.wifi.tx_power_corrections = outcome.point_corrections;
        self.wifi.tx_power_adjustment = outcome.power_adjustment;
        self.common.calibrated_attenuation = outcome.final_attenuation;
        self.common.clear_tone_after_ready = false;
        self.wifi
            .calibration
            .set(WifiCalibrationStatus::TX_POWER, true);
        self.wifi.current_channel = outcome.current_channel;
    }

    pub const fn tx_dc_pwdet_parameters(&self) -> PhyTxDcPwdetParameters {
        PhyTxDcPwdetParameters {
            dco: [
                self.wifi.tx_dco[0],
                self.wifi.tx_dco[1],
                self.wifi.tx_dco[2],
            ],
            clear_tone_after_ready: self.common.clear_tone_after_ready,
        }
    }

    pub fn apply_tx_dc_pwdet_outcome(&mut self, outcome: PhyTxDcPwdetOutcome) {
        self.wifi.tx_dco[0] = outcome.dco[0];
        self.wifi.tx_dco[1] = outcome.dco[1];
        self.wifi.tx_dco[2] = outcome.dco[2];
    }

    pub const fn tx_iq_parameters(&self) -> PhyTxIqInitParameters {
        PhyTxIqInitParameters {
            already_calibrated: self.wifi.calibration.contains(WifiCalibrationStatus::TX_IQ),
            crystal_selector: self.common.crystal_selector,
            environment: PhyTxCalibrationParameters {
                pbus_tx_path_value: self.config.tx_power_path,
                pbus_rx_path_value: self.config.pbus_rx_path,
                dco: self.wifi.tx_dco[0],
            },
            capacitance: self.wifi.tx_capacitance,
            channel_6_dcode: [self.common.dcode[2], self.common.dcode[3]],
            initial_attenuation: self.common.calibrated_attenuation as i8,
            power_offset: self.config.power_offset,
            reference_codes: self.wifi.tx_reference_codes,
            clear_tone_after_ready: self.common.clear_tone_after_ready,
        }
    }

    pub fn apply_tx_iq_outcome(&mut self, outcome: PhyTxIqInitOutcome) {
        if !outcome.calibration_performed {
            return;
        }
        self.wifi.external_dcode = outcome.external_dcode;
        self.wifi.tx_iq_config = outcome.coefficient[0];
        self.wifi.tx_iq_coefficient = outcome.coefficient[1];
        if let Some(temperature) = outcome.temperature {
            self.wifi.calibration_temperature = temperature.temperature;
            self.apply_temperature_outcome(temperature);
        }
        self.wifi
            .calibration
            .set(WifiCalibrationStatus::TX_IQ, true);
    }

    pub const fn rx_iq_parameters(&self) -> PhyRxIqInitParameters {
        PhyRxIqInitParameters {
            crystal_selector: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
            capacitance: self.wifi.tx_capacitance,
            channel_6_dcode: [self.common.dcode[2], self.common.dcode[3]],
            adjusted_tx: PhyRxIqAdjustedTxParameters {
                coefficient: self.wifi.tx_iq_coefficient,
                current_channel: self.wifi.current_channel,
                current_temperature: self.common.temperature as u16,
                calibration_temperature: self.wifi.calibration_temperature as u16,
                calibration_dcode: self.wifi.external_dcode,
            },
            coefficients: self.wifi.rx_iq_coefficients,
        }
    }

    pub fn apply_rx_iq_outcome(&mut self, outcome: PhyRxIqInitOutcome) {
        self.wifi.rx_iq_coefficients = outcome.coefficients;
        self.wifi.current_channel = outcome.current_channel;
    }

    const fn wifi_seed(&self) -> [u32; 6] {
        let rows = [
            self.wifi.tx_dco[0],
            self.wifi.tx_dco[1],
            self.wifi.tx_dco[2],
        ];
        Self::packed_seed(&rows)
    }

    pub const fn channel_parameters(&self) -> PhyChipChannelParameters {
        PhyChipChannelParameters {
            frequency_offset: 0,
            crystal_selector: self.common.crystal_selector,
            channel_14_mic_enabled: self.config.channel_14_mic_enabled,
            tx_gain_skip_publication: self.config.tx_gain_skip_publication,
            tx_gain_seed: self.wifi_seed(),
            tx_gain_config: self.wifi.tx_iq_config,
            tx_gain_curve: [
                self.wifi.tx_power_curve[0] as u8,
                self.wifi.tx_power_curve[1] as u8,
                self.wifi.tx_power_curve[2] as u8,
                self.wifi.tx_power_corrections[0] as u8,
                self.wifi.tx_power_corrections[1] as u8,
                self.wifi.tx_power_corrections[2] as u8,
            ],
            tx_gain_correction: self.wifi.tx_power_adjustment,
            tx_gain_base: self.wifi.tracking_gain_base,
            tx_gain_adjustment: self.wifi.tx_gain_adjustment,
            tx_capacitance: self.wifi.tx_capacitance,
        }
    }

    /// Channel selected by the completed typed channel transition.
    pub const fn current_wifi_channel(&self) -> u16 {
        self.wifi.current_channel
    }

    /// Channel-width image last committed by the typed channel transition.
    pub const fn current_wifi_channel_bandwidth(&self) -> u8 {
        self.wifi.channel_bandwidth
    }

    /// Regenerate the Wi-Fi gain bank from live calibration state.
    ///
    /// `None` preserves the vendor's explicit skip-publication configuration;
    /// the surrounding tracking child still completes normally in that case.
    pub const fn wifi_tracking_gain_image(
        &self,
        channel: u16,
        gain_base: i8,
    ) -> Option<PhyWifiTxGainImage> {
        let parameters = self.channel_parameters();
        if parameters.tx_gain_skip_publication {
            return None;
        }
        let mut image = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
            channel,
            calibration_curve: parameters.tx_gain_curve,
            correction: parameters.tx_gain_correction,
            base_and_delta: (gain_base as u8).wrapping_add(parameters.tx_gain_adjustment as u8)
                as i8,
        });
        image.seed = parameters.tx_gain_seed;
        image.config = parameters.tx_gain_config;
        Some(image)
    }

    /// Regenerate the Wi-Fi calibration gain bank from a just-completed
    /// TXDC/PWDET result that has not yet reached the outer state commit.
    ///
    /// The periodic parent invokes the gain publisher immediately after TXDC
    /// and commits both effects only after restoring the RF guards. Using the
    /// retained state seed here would therefore republish the previous DCO.
    pub const fn wifi_calibration_gain_image(
        &self,
        channel: u16,
        tx_dc_pwdet: PhyTxDcPwdetOutcome,
    ) -> Option<PhyWifiTxGainImage> {
        let mut image =
            match self.wifi_tracking_gain_image(channel, self.wifi.tracking_gain_base as i8) {
                Some(image) => image,
                None => return None,
            };
        image.seed = Self::packed_seed(&tx_dc_pwdet.dco);
        Some(image)
    }

    pub fn apply_channel_outcome(&mut self, outcome: PhyChipChannelOutcome) {
        self.wifi.current_channel = outcome.channel;
        self.wifi.channel_initialized = outcome.init_complete;
        self.wifi.channel_bandwidth = outcome.cbw;
        self.apply_temperature_outcome(outcome.temperature);
    }

    pub fn apply_rx_saturation_outcome(
        &mut self,
        outcome: PhyRxSaturationOutcome,
    ) -> Result<(), PhyRxSaturationOutcome> {
        match outcome {
            PhyRxSaturationOutcome::Measured {
                saturated_samples, ..
            } => {
                if saturated_samples != 0 {
                    self.wifi
                        .calibration
                        .set(WifiCalibrationStatus::RX_SATURATION, true);
                }
                Ok(())
            }
            failure => Err(failure),
        }
    }

    pub fn begin_full_calibration(&mut self, config: PhyConfig) {
        self.common.calibrated_attenuation = config.initial_calibration_attenuation;
        self.wifi.tracking_gain_base = config.initial_wifi_tx_gain_base;
        self.bluetooth.tracking_gain_base = config.initial_bluetooth_tx_gain_base;
        self.config = config;
        self.common.crystal_selector = 0;
        self.clear_calibration_status();
    }

    /// Restore semantic calibration products for a cold partial-calibration
    /// run without retaining runtime ownership or hardware-resident flags.
    ///
    /// The caller validates schema and physical identity before invoking this
    /// transition. A fresh state is assembled locally and published only after
    /// all fields have been projected, so a rejected cache cannot partially
    /// modify the live owner.
    pub(crate) fn begin_cached_calibration(
        &mut self,
        config: PhyConfig,
        cache: &PhyCalibrationCache,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> Result<(), PhyCalibrationCacheError> {
        cache.validate_for_replay(identity)?;
        let snapshot = cache.snapshot;
        let mut restored = Self::new(config);

        restored.common.temperature = snapshot.common.temperature;
        restored.common.references = TemperatureReferences::restored(
            snapshot.common.rfpll_reference_temperature,
            snapshot.common.rxcal_reference_temperature,
            snapshot.common.txcal_reference_temperature,
            snapshot.common.power_reference_temperature,
        );
        restored.common.calibrated_attenuation = snapshot.wifi.calibrated_attenuation;
        restored.common.sensor_index = snapshot.common.sensor_index;
        restored.common.crystal_selector = snapshot.common.crystal_selector;
        restored.common.rc_result = snapshot.common.rc_result;
        restored.common.filter_dcap = snapshot.common.filter_dcap;
        restored.common.rc_calibrated = snapshot.common.rc_calibrated;
        restored.common.dcode = snapshot.common.dcode;
        restored.common.i2c_frequency_parameter = snapshot.common.i2c_frequency_parameter;
        restored.common.xtal_duty = snapshot.common.xtal_duty;
        restored.common.clear_tone_after_ready = snapshot.common.clear_tone_after_ready;

        restored.wifi.tx_gain_adjustment = snapshot.wifi.tx_gain_adjustment;
        restored.wifi.calibration.set(
            WifiCalibrationStatus::BASEBAND,
            snapshot.wifi.baseband_calibrated,
        );
        restored
            .wifi
            .calibration
            .set(WifiCalibrationStatus::PWDET, snapshot.wifi.pwdet_calibrated);
        restored.wifi.calibration.set(
            WifiCalibrationStatus::TX_POWER,
            snapshot.wifi.tx_power_calibrated,
        );
        restored
            .wifi
            .calibration
            .set(WifiCalibrationStatus::TX_IQ, snapshot.wifi.tx_iq_calibrated);
        restored.wifi.calibration.set(
            WifiCalibrationStatus::RX_GAIN_DC,
            snapshot.wifi.rx_gain_dc_calibrated,
        );
        // A cache proves that coefficients exist, not that the cold hardware
        // bank still contains them. Force the ordinary publisher to rebuild it.
        restored
            .wifi
            .calibration
            .set(WifiCalibrationStatus::RX_GAIN_TABLES, false);
        restored.wifi.calibration.set(
            WifiCalibrationStatus::RX_SATURATION,
            snapshot.wifi.rx_saturation_detected,
        );
        restored.wifi.tx_dco = snapshot.wifi.tx_dco;
        restored.wifi.tx_reference_codes = snapshot.wifi.tx_reference_codes;
        restored.wifi.tx_capacitance = snapshot.wifi.tx_capacitance;
        restored.wifi.tx_power_curve = snapshot.wifi.tx_power_curve;
        restored.wifi.tx_power_corrections = snapshot.wifi.tx_power_corrections;
        restored.wifi.tx_power_adjustment = snapshot.wifi.tx_power_adjustment;
        restored.wifi.tx_iq_config = snapshot.wifi.tx_iq_config;
        restored.wifi.tx_iq_coefficient = snapshot.wifi.tx_iq_coefficient;
        restored.wifi.rx_iq_coefficients = snapshot.wifi.rx_iq_coefficients;
        restored.wifi.external_dcode = snapshot.wifi.external_dcode;
        restored.wifi.calibration_temperature = snapshot.wifi.calibration_temperature;
        restored.wifi.current_channel = snapshot.wifi.calibration_channel;
        restored.wifi.wifi_rx_table_last_index = snapshot.wifi.wifi_rx_table_last_index;
        restored.wifi.shared_rx_table_last_index = snapshot.wifi.shared_rx_table_last_index;
        restored.wifi.wifi_index_dc = snapshot.wifi.wifi_index_dc;
        restored.wifi.wifi_fine_dc = snapshot.wifi.wifi_fine_dc;
        restored.wifi.shared_index_dc = snapshot.wifi.shared_index_dc;

        restored.bluetooth.tx_dc_calibrated = snapshot.bluetooth.tx_dc_calibrated;
        restored.bluetooth.tx_power_calibrated = snapshot.bluetooth.tx_power_calibrated;
        restored.bluetooth.tx_dco = snapshot.bluetooth.tx_dco;
        restored.bluetooth.tx_power_curve = snapshot.bluetooth.tx_power_curve;
        restored.bluetooth.tx_power_corrections = snapshot.bluetooth.tx_power_corrections;
        restored.bluetooth.tx_power_adjustment = snapshot.bluetooth.tx_power_adjustment;

        // Frequency memory and channel state are physical-epoch products. The
        // cold RF graph regenerates them from retained calibration parameters.
        restored.common.frequency_table_initialized = false;
        restored.wifi.channel_initialized = false;
        restored.common.registered = false;
        *self = restored;
        Ok(())
    }

    /// The state retaining exactly `cache` (`cfg(oer_probe)`): the replay
    /// restoration, with the RX-gain table flag the replay deliberately
    /// clears kept as captured.
    #[cfg(oer_probe)]
    pub(crate) fn retained_calibration(
        config: PhyConfig,
        cache: &PhyCalibrationCache,
    ) -> Result<Self, PhyCalibrationCacheError> {
        let mut state = Self::new(config);
        state.begin_cached_calibration(config, cache, cache.snapshot.identity)?;
        state.wifi.calibration.set(
            WifiCalibrationStatus::RX_GAIN_TABLES,
            cache.snapshot.wifi.rx_gain_tables_initialized,
        );
        // The snapshot is captured after registration, whose temperature
        // step restarts TX-power tracking from the TX-power reference
        // (`apply_register_temperature_outcome`, as `phy_get_temp_init`).
        state.common.references.restart_power_tracking();
        Ok(state)
    }

    /// The calibration this state retains, as persistence would capture it,
    /// under an all-zero identity: what a vendor comparison or a hardware
    /// cross-check reads the committed fields from.
    #[cfg(any(test, oer_probe))]
    pub const fn retained_calibration_snapshot(&self) -> PhyCalibrationSnapshot {
        self.calibration_snapshot(crate::calibration::registration::PhyCalibrationIdentity {
            rf_cal_version: 0,
            base_mac_address: [0; 6],
            mac_extension: 0,
        })
    }

    /// The reference temperature RFPLL tracking compares against.
    pub const fn rfpll_reference_temperature(&self) -> i16 {
        self.rfpll_tracking_request(None).reference_temperature
    }

    pub(crate) const fn calibration_snapshot(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> PhyCalibrationSnapshot {
        PhyCalibrationSnapshot {
            schema: PHY_CALIBRATION_SNAPSHOT_SCHEMA,
            identity,
            common: PhyCommonCalibration {
                temperature: self.common.temperature,
                rfpll_reference_temperature: self.common.references.rfpll(),
                rxcal_reference_temperature: self.common.references.common(),
                txcal_reference_temperature: self.common.references.transmit(),
                power_reference_temperature: self.common.references.power(),
                sensor_index: self.common.sensor_index,
                crystal_selector: self.common.crystal_selector,
                rc_result: self.common.rc_result,
                filter_dcap: self.common.filter_dcap,
                rc_calibrated: self.common.rc_calibrated,
                dcode: self.common.dcode,
                i2c_frequency_parameter: self.common.i2c_frequency_parameter,
                xtal_duty: self.common.xtal_duty,
                clear_tone_after_ready: self.common.clear_tone_after_ready,
            },
            wifi: PhyWifiCalibration {
                tx_gain_adjustment: self.wifi.tx_gain_adjustment,
                baseband_calibrated: self
                    .wifi
                    .calibration
                    .contains(WifiCalibrationStatus::BASEBAND),
                pwdet_calibrated: self.wifi.calibration.contains(WifiCalibrationStatus::PWDET),
                tx_power_calibrated: self
                    .wifi
                    .calibration
                    .contains(WifiCalibrationStatus::TX_POWER),
                tx_iq_calibrated: self.wifi.calibration.contains(WifiCalibrationStatus::TX_IQ),
                rx_gain_dc_calibrated: self
                    .wifi
                    .calibration
                    .contains(WifiCalibrationStatus::RX_GAIN_DC),
                rx_gain_tables_initialized: self
                    .wifi
                    .calibration
                    .contains(WifiCalibrationStatus::RX_GAIN_TABLES),
                rx_saturation_detected: self
                    .wifi
                    .calibration
                    .contains(WifiCalibrationStatus::RX_SATURATION),
                tx_dco: self.wifi.tx_dco,
                tx_reference_codes: self.wifi.tx_reference_codes,
                tx_capacitance: self.wifi.tx_capacitance,
                tx_power_curve: self.wifi.tx_power_curve,
                tx_power_corrections: self.wifi.tx_power_corrections,
                tx_power_adjustment: self.wifi.tx_power_adjustment,
                calibrated_attenuation: self.common.calibrated_attenuation,
                tx_iq_config: self.wifi.tx_iq_config,
                tx_iq_coefficient: self.wifi.tx_iq_coefficient,
                rx_iq_coefficients: self.wifi.rx_iq_coefficients,
                external_dcode: self.wifi.external_dcode,
                calibration_temperature: self.wifi.calibration_temperature,
                calibration_channel: self.wifi.current_channel,
                wifi_rx_table_last_index: self.wifi.wifi_rx_table_last_index,
                shared_rx_table_last_index: self.wifi.shared_rx_table_last_index,
                wifi_index_dc: self.wifi.wifi_index_dc,
                wifi_fine_dc: self.wifi.wifi_fine_dc,
                shared_index_dc: self.wifi.shared_index_dc,
            },
            bluetooth: PhyBluetoothCalibration {
                tx_dc_calibrated: self.bluetooth.tx_dc_calibrated,
                tx_power_calibrated: self.bluetooth.tx_power_calibrated,
                tx_dco: self.bluetooth.tx_dco,
                tx_power_curve: self.bluetooth.tx_power_curve,
                tx_power_corrections: self.bluetooth.tx_power_corrections,
                tx_power_adjustment: self.bluetooth.tx_power_adjustment,
            },
        }
    }

    pub(crate) const fn calibration_cache(
        &self,
        identity: crate::calibration::registration::PhyCalibrationIdentity,
    ) -> PhyCalibrationCache {
        PhyCalibrationCache::capture(identity, self)
    }

    pub const fn register_temperature_control(&self) -> PhyRegisterTemperatureControl {
        PhyRegisterTemperatureControl {
            update_registration_reference: !self.common.frequency_table_initialized,
            update_calibration_reference: !self
                .wifi
                .calibration
                .contains(WifiCalibrationStatus::TX_POWER),
        }
    }

    /// Commit the registration temperature step, as the pinned libphy
    /// `phy_get_temp_init` does: before the TX-power calibration the sample
    /// becomes the TX-power reference (`phy_param[302]`) and the TX reference
    /// (`phy_param[72]`); every registration then restarts TX-power tracking
    /// from that reference (`phy_param[4]`).
    pub fn apply_register_temperature_outcome(
        &mut self,
        control: PhyRegisterTemperatureControl,
        outcome: PhyTemperatureOutcome,
    ) {
        self.apply_temperature_outcome(outcome);
        self.common
            .references
            .commit_registration(outcome.temperature, control);
    }

    pub fn apply_full_calibration_temperature(&mut self, outcome: PhyTemperatureOutcome) {
        self.apply_register_temperature_outcome(PhyRegisterTemperatureControl::FULL, outcome);
    }

    fn clear_calibration_status(&mut self) {
        self.common.rc_calibrated = false;
        self.common.frequency_table_initialized = false;
        self.wifi.calibration = WifiCalibrationStatus::EMPTY;
        self.bluetooth.tx_dc_calibrated = false;
        self.bluetooth.tx_power_calibrated = false;
    }

    pub(crate) fn mark_phy_registered(&mut self) {
        self.common.registered = true;
    }

    pub const fn phy_registered(&self) -> bool {
        self.common.registered
    }

    pub const fn rc_calibration_complete(&self) -> bool {
        self.common.rc_calibrated
    }

    /// Derive the five filter-DCAP parameter bytes from one RC-calibration
    /// result.
    ///
    /// SOURCE(esp32s31): rev0 ROM `phy_rc_cal` computes each code as
    /// `82 * (result + 56) / (10 * divisor) - 8` (the auxiliary codes as
    /// `0x334 * (result + 56) / (104 * divisor) - 8`) and saturates it, after
    /// bounding a result above 45 to 50. `libphy.a[phy_init.o]::phy_rc_cal_init`
    /// passes the divisor, bound and auxiliary byte arrays retained below.
    /// The two primary divisors are 20 and 40, equal to the 20 and 40 MHz
    /// Wi-Fi channel widths, and a code falls as its divisor rises; these
    /// bodies do not establish the bandwidth each code produces.
    pub fn apply_rc_calibration(&mut self, result: u8) {
        const UPPER: [u8; 4] = [0x28, 0x14, 0x1e, 0x14];
        const PRIMARY: [u8; 2] = [0x14, 0x28];
        const AUX: [u8; 4] = [0x24, 0x28, 0x16, 0x20];
        let bounded = if result > 45 { 50 } else { result };
        let base = bounded as i32 + 56;
        self.common.rc_result = result;
        let mut index = 0;
        while index != PRIMARY.len() {
            let value = base * 82 / (PRIMARY[index] as i32 * 10) - 8;
            self.common.filter_dcap[index] = saturate_phy_value(value, UPPER[index], 2);
            index += 1;
        }
        index = 0;
        while index != AUX.len() {
            let value = base * 0x334 / (AUX[index] as i32 * 104) - 8;
            let slot = match index {
                0 => 2,
                1 => 3,
                3 => 4,
                _ => {
                    index += 1;
                    continue;
                }
            };
            self.common.filter_dcap[slot] = saturate_phy_value(value, UPPER[2 + (index & 1)], 0);
            index += 1;
        }
        self.common.rc_calibrated = true;
    }

    pub const fn filter_dcap_parameters(&self) -> FilterDcapParameters {
        FilterDcapParameters::new(
            self.common.filter_dcap[0],
            self.common.filter_dcap[1],
            self.common.filter_dcap[2],
            self.common.filter_dcap[3],
            self.common.filter_dcap[4],
        )
    }

    /// Return the retained parameter snapshot consumed by the wake I²C and
    /// frequency-publication children.
    pub const fn rf_init_parameter_snapshot(&self) -> PhyRfInitParameterSnapshot {
        PhyRfInitParameterSnapshot::new(
            self.filter_dcap_parameters(),
            self.common.i2c_frequency_parameter,
        )
    }

    pub const fn xtal_duty_parameters(&self) -> XtalDutyCalibrationParameters {
        XtalDutyCalibrationParameters {
            restore_duty: self.common.xtal_duty[1],
            rf_frequency_offset_base: self.common.crystal_selector,
            pbus_rx_path_value: self.config.pbus_rx_path,
        }
    }

    pub const fn channel_frequency_control(&self) -> PhyChannelFrequencyInitControl {
        PhyChannelFrequencyInitControl {
            frequency_register_parameter_override: self.bluetooth.channel_base != 0,
            frequency_table_initialized: self.common.frequency_table_initialized,
            front_end_parameter_bit: self.common.front_end_parameter,
        }
    }

    pub fn set_xtal_frequency_mhz(&mut self, frequency_mhz: u32) {
        self.common.crystal_selector = match frequency_mhz {
            26 => 1,
            32 => 2,
            _ => 0,
        };
    }

    pub(crate) fn synchronize_success(&mut self, outcome: PhyRfInitPrefixOutcome) {
        let PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
            parameter,
            xtal_duty,
            channel_frequency,
            ..
        } = outcome
        else {
            return;
        };
        self.common.i2c_frequency_parameter = parameter.parameter_18e();
        self.common.xtal_duty = [
            xtal_duty.initial_duty,
            xtal_duty.low_frequency.best_candidate,
            xtal_duty.high_frequency.best_candidate,
        ];
        self.common.frequency_table_initialized = channel_frequency.table_is_initialized;
    }
}

impl Default for PhyState {
    fn default() -> Self {
        Self::new(PhyConfig::esp32s31_default())
    }
}

#[cfg(test)]
mod tests;

pub mod client;
