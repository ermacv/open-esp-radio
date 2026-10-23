//! Fixed little-endian semantic calibration record for ESP32-S31.
//!
//! This vocabulary mirrors snapshot schema 8 without depending on the PHY or
//! HAL. The firmware adapter maps fields explicitly. Decode is structural:
//! physical identity, completion flags and RX table validity remain the native
//! registration boundary's responsibility. No received value is repaired.

/// Opaque format identifier, ASCII `S3C1` in little-endian order.
pub const S31_SEMANTIC_V1: u32 = u32::from_le_bytes(*b"S3C1");
/// Snapshot schema encoded by this version of the record.
pub const SNAPSHOT_SCHEMA: u16 = 8;
/// Exact record length, with no native padding or pointer representation.
pub const RECORD_BYTES: usize = 261;
/// Reviewed calibration/replay algorithm revision, independent of the RF ABI.
pub const CALIBRATION_ALGORITHM_REVISION: u32 = 2;
/// Reviewed `PhyConfig::production()` profile, independent of the image commit.
pub const CALIBRATION_PROFILE_ID: u32 = 1;

/// A structural record error, before native replay validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// The format belongs to another codec or revision.
    Format,
    /// The record is truncated or has trailing bytes.
    Length,
    /// The snapshot schema is unsupported.
    Schema,
    /// Boolean values must be exactly zero or one.
    Boolean,
}

// Keep cursor operations out of line: one bounded operation is shared across
// fields and array elements instead of expanding the codec into each caller.
// Field order here is the format contract. Arrays are row-major; scalar
// integers have their fixed width, signed values use two's complement.
macro_rules! record {
    ($(#[$doc:meta])* $name:ident { $($(#[$field_doc:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
        pub struct $name { $($(#[$field_doc])* pub $field: $ty),* }
        impl Value for $name {
            #[inline(never)]
            fn put(&self, output: &mut &mut [u8]) -> Result<(), CodecError> {
                $(self.$field.put(output)?;)*
                Ok(())
            }
            #[inline(never)]
            fn take(input: &mut &[u8]) -> Result<Self, CodecError> {
                Ok(Self { $($field: <$ty>::take(input)?),* })
            }
        }
    };
}

record! {
    /// Physical RF ABI and eFuse identity, encoded in 12 bytes.
    CalibrationIdentity {
        /// RF calibration data ABI, independent of the OER algorithm revision.
        rf_cal_version: u32,
        /// Six eFuse base-MAC bytes in their original order.
        base_mac_address: [u8; 6],
        /// eFuse MAC extension, little-endian.
        mac_extension: u16,
    }
}

record! {
    /// Common analog calibration products and reference temperatures.
    CommonCalibration {
        /// Most recently observed temperature in degrees Celsius; not a replay guard.
        temperature: i16,
        /// RFPLL tracking reference temperature in degrees Celsius.
        rfpll_reference_temperature: i16,
        /// Common RX tracking reference temperature in degrees Celsius.
        rxcal_reference_temperature: i16,
        /// Transmit tracking reference temperature in degrees Celsius.
        txcal_reference_temperature: i16,
        /// TX-power calibration reference temperature in degrees Celsius.
        power_reference_temperature: i16,
        /// Sensor index.
        sensor_index: u8,
        /// Crystal selector.
        crystal_selector: u8,
        /// Rc result.
        rc_result: u8,
        /// Filter dcap.
        filter_dcap: [u8; 5],
        /// Rc calibrated.
        rc_calibrated: bool,
        /// Dcode.
        dcode: [u8; 8],
        /// I2c frequency parameter.
        i2c_frequency_parameter: u8,
        /// Xtal duty.
        xtal_duty: [u8; 3],
        /// Clear tone after ready.
        clear_tone_after_ready: bool,
    }
}

record! {
    /// Wi-Fi calibration products, completion guards and RX table shape.
    WifiCalibration {
        /// Independent signed additive Wi-Fi gain input.
        tx_gain_adjustment: i8,
        /// Baseband calibrated.
        baseband_calibrated: bool,
        /// Pwdet calibrated.
        pwdet_calibrated: bool,
        /// Tx power calibrated.
        tx_power_calibrated: bool,
        /// Tx iq calibrated.
        tx_iq_calibrated: bool,
        /// Rx gain dc calibrated.
        rx_gain_dc_calibrated: bool,
        /// Rx gain tables initialized.
        rx_gain_tables_initialized: bool,
        /// Rx saturation detected.
        rx_saturation_detected: bool,
        /// Tx dco.
        tx_dco: [[u16; 4]; 5],
        /// Tx reference codes.
        tx_reference_codes: [i16; 2],
        /// Tx capacitance.
        tx_capacitance: [u8; 6],
        /// Tx power curve.
        tx_power_curve: [i8; 3],
        /// Tx power corrections.
        tx_power_corrections: [i8; 3],
        /// Tx power adjustment.
        tx_power_adjustment: i8,
        /// Calibrated attenuation.
        calibrated_attenuation: u8,
        /// Tx iq config.
        tx_iq_config: u16,
        /// Tx iq coefficient.
        tx_iq_coefficient: u16,
        /// Rx iq coefficients.
        rx_iq_coefficients: [u16; 4],
        /// External dcode.
        external_dcode: [u8; 2],
        /// Calibration temperature.
        calibration_temperature: i16,
        /// Calibration channel.
        calibration_channel: u16,
        /// Wifi rx table last index.
        wifi_rx_table_last_index: u8,
        /// Shared rx table last index.
        shared_rx_table_last_index: u8,
        /// Wifi index dc.
        wifi_index_dc: [[u16; 2]; 8],
        /// I/Q DC pairs for the five fine-gain codes.
        wifi_fine_dc: [[u16; 2]; 5],
        /// Shared index dc.
        shared_index_dc: [[u16; 2]; 11],
    }
}

record! {
    /// Bluetooth calibration products and completion guards.
    BluetoothCalibration {
        /// Tx dc calibrated.
        tx_dc_calibrated: bool,
        /// Tx power calibrated.
        tx_power_calibrated: bool,
        /// Tx dco.
        tx_dco: [[u16; 4]; 3],
        /// Tx power curve.
        tx_power_curve: [i8; 3],
        /// Tx power corrections.
        tx_power_corrections: [i8; 3],
        /// Tx power adjustment.
        tx_power_adjustment: i8,
    }
}

record! {
    /// Complete S31 semantic record; every schema-8 snapshot field is preserved.
    CalibrationRecord {
        /// Native snapshot schema; V1 accepts only schema 8.
        schema: u16,
        /// Identity.
        identity: CalibrationIdentity,
        /// Common.
        common: CommonCalibration,
        /// Wifi.
        wifi: WifiCalibration,
        /// Bluetooth.
        bluetooth: BluetoothCalibration,
    }
}

impl CalibrationIdentity {
    /// Canonical identity bytes for an opaque transport/file key.
    pub fn encode(self) -> [u8; 12] {
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&self.rf_cal_version.to_le_bytes());
        bytes[4..10].copy_from_slice(&self.base_mac_address);
        bytes[10..].copy_from_slice(&self.mac_extension.to_le_bytes());
        bytes
    }
}

impl CalibrationRecord {
    /// Encode all fields without clamping or inferring completion.
    pub fn encode(&self) -> Result<[u8; RECORD_BYTES], CodecError> {
        if self.schema != SNAPSHOT_SCHEMA {
            return Err(CodecError::Schema);
        }
        let mut bytes = [0; RECORD_BYTES];
        let mut output = &mut bytes[..];
        self.put(&mut output)?;
        if !output.is_empty() {
            return Err(CodecError::Length);
        }
        Ok(bytes)
    }

    /// Reject foreign formats, noncanonical booleans, schemas and lengths.
    /// Native identity/completion/RX validation must precede replay.
    pub fn decode(format: u32, bytes: &[u8]) -> Result<Self, CodecError> {
        if format != S31_SEMANTIC_V1 {
            return Err(CodecError::Format);
        }
        if bytes.len() != RECORD_BYTES {
            return Err(CodecError::Length);
        }
        let mut input = bytes;
        let record = Self::take(&mut input)?;
        if !input.is_empty() {
            return Err(CodecError::Length);
        }
        if record.schema != SNAPSHOT_SCHEMA {
            return Err(CodecError::Schema);
        }
        Ok(record)
    }
}

trait Value: Sized {
    #[inline(never)]
    fn put(&self, output: &mut &mut [u8]) -> Result<(), CodecError>;
    #[inline(never)]
    fn take(input: &mut &[u8]) -> Result<Self, CodecError>;
}

macro_rules! integer {
    ($($ty:ty),*) => {$(
        impl Value for $ty {
            #[inline(never)]
            fn put(&self, output: &mut &mut [u8]) -> Result<(), CodecError> {
                let width = core::mem::size_of::<Self>();
                let bytes = core::mem::take(output);
                let (head, tail) = bytes.split_at_mut_checked(width).ok_or(CodecError::Length)?;
                head.copy_from_slice(&self.to_le_bytes());
                *output = tail;
                Ok(())
            }
            #[inline(never)]
            fn take(input: &mut &[u8]) -> Result<Self, CodecError> {
                let width = core::mem::size_of::<Self>();
                let (head, tail) = input.split_at_checked(width).ok_or(CodecError::Length)?;
                *input = tail;
                Ok(Self::from_le_bytes(head.try_into().map_err(|_| CodecError::Length)?))
            }
        }
    )*};
}
integer!(u8, i8, u16, i16, u32);

impl Value for bool {
    #[inline(never)]
    fn put(&self, output: &mut &mut [u8]) -> Result<(), CodecError> {
        u8::from(*self).put(output)
    }
    #[inline(never)]
    fn take(input: &mut &[u8]) -> Result<Self, CodecError> {
        match u8::take(input)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(CodecError::Boolean),
        }
    }
}

impl<T: Value + Default + Copy, const N: usize> Value for [T; N] {
    #[inline(never)]
    fn put(&self, output: &mut &mut [u8]) -> Result<(), CodecError> {
        for value in self {
            value.put(output)?;
        }
        Ok(())
    }
    #[inline(never)]
    fn take(input: &mut &[u8]) -> Result<Self, CodecError> {
        let mut values = [T::default(); N];
        for value in &mut values {
            *value = T::take(input)?;
        }
        Ok(values)
    }
}

#[cfg(test)]
mod revision_guard;
#[cfg(test)]
mod tests;
