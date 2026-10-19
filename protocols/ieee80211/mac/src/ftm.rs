//! Allocation-free IEEE 802.11 Fine Timing Measurement Action codecs.
//!
//! The field order and assigned values in this module follow IEEE Std
//! 802.11-2020, 9.4.2.167 and 9.6.7.1/32/33. This codec deliberately stops at
//! the Action body. A MAC owner remains responsible for the management header,
//! sequence control, FCS, protection policy and physical publication.

pub const PUBLIC_ACTION_CATEGORY: u8 = 4;
pub const FTM_REQUEST_PUBLIC_ACTION: u8 = 32;
pub const FTM_MEASUREMENT_PUBLIC_ACTION: u8 = 33;
pub const FTM_PARAMETERS_ELEMENT_ID: u8 = 206;
pub const FTM_PARAMETERS_FIELD_LEN: usize = 9;
pub const FTM_PARAMETERS_ELEMENT_LEN: usize = 2 + FTM_PARAMETERS_FIELD_LEN;
pub const FTM_REQUEST_PREFIX_LEN: usize = 3;
pub const FTM_INITIAL_REQUEST_BODY_LEN: usize = FTM_REQUEST_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN;
pub const FTM_MEASUREMENT_PREFIX_LEN: usize = 20;

const FTM_TIMESTAMP_MASK: u64 = (1_u64 << 48) - 1;
const MAX_UNAMBIGUOUS_PARTIAL_TSF: u16 = 63_487;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmParameterError {
    WrongElementId,
    WrongElementLength,
    ReservedBitsSet,
    ReservedStatus,
    ValueOutOfRange,
    InvalidBurstDuration,
    InvalidFormatAndBandwidth,
    PartialTsfOutOfRange,
    BurstPeriodReserved,
    RequestAsapCapableSet,
    ResponseNoPreference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmWireError {
    BodyTooShort,
    WrongCategory,
    WrongPublicAction,
    ReservedTrigger,
    OutputTooShort,
    MalformedInformationElement,
    DuplicateParametersElement,
    Parameters(FtmParameterError),
    ReservedTimestampFields,
    ReservedTimestampErrorBits,
    TimestampOutOfRange,
}

impl From<FtmParameterError> for FtmWireError {
    fn from(error: FtmParameterError) -> Self {
        Self::Parameters(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmTrigger {
    Stop,
    StartOrContinue,
}

impl FtmTrigger {
    const fn wire(self) -> u8 {
        match self {
            Self::Stop => 0,
            Self::StartOrContinue => 1,
        }
    }

    const fn from_wire(value: u8) -> Result<Self, FtmWireError> {
        match value {
            0 => Ok(Self::Stop),
            1 => Ok(Self::StartOrContinue),
            _ => Err(FtmWireError::ReservedTrigger),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FtmBurstDuration {
    Micros250 = 2,
    Micros500 = 3,
    Millis1 = 4,
    Millis2 = 5,
    Millis4 = 6,
    Millis8 = 7,
    Millis16 = 8,
    Millis32 = 9,
    Millis64 = 10,
    Millis128 = 11,
    NoPreference = 15,
}

impl FtmBurstDuration {
    const fn from_wire(value: u8) -> Result<Self, FtmParameterError> {
        match value {
            2 => Ok(Self::Micros250),
            3 => Ok(Self::Micros500),
            4 => Ok(Self::Millis1),
            5 => Ok(Self::Millis2),
            6 => Ok(Self::Millis4),
            7 => Ok(Self::Millis8),
            8 => Ok(Self::Millis16),
            9 => Ok(Self::Millis32),
            10 => Ok(Self::Millis64),
            11 => Ok(Self::Millis128),
            15 => Ok(Self::NoPreference),
            _ => Err(FtmParameterError::InvalidBurstDuration),
        }
    }

    pub const fn wire(self) -> u8 {
        self as u8
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FtmFormatAndBandwidth {
    NoPreference = 0,
    NonHt5Mhz = 4,
    NonHt10Mhz = 6,
    NonHt20Mhz = 8,
    HtMixed20Mhz = 9,
    Vht20Mhz = 10,
    HtMixed40Mhz = 11,
    Vht40Mhz = 12,
    Vht80Mhz = 13,
    Vht80Plus80Mhz = 14,
    Vht160MhzSeparateLos = 15,
    Vht160MhzSingleLo = 16,
    Dmg2160Mhz = 31,
}

impl FtmFormatAndBandwidth {
    const fn from_wire(value: u8) -> Result<Self, FtmParameterError> {
        match value {
            0 => Ok(Self::NoPreference),
            4 => Ok(Self::NonHt5Mhz),
            6 => Ok(Self::NonHt10Mhz),
            8 => Ok(Self::NonHt20Mhz),
            9 => Ok(Self::HtMixed20Mhz),
            10 => Ok(Self::Vht20Mhz),
            11 => Ok(Self::HtMixed40Mhz),
            12 => Ok(Self::Vht40Mhz),
            13 => Ok(Self::Vht80Mhz),
            14 => Ok(Self::Vht80Plus80Mhz),
            15 => Ok(Self::Vht160MhzSeparateLos),
            16 => Ok(Self::Vht160MhzSingleLo),
            31 => Ok(Self::Dmg2160Mhz),
            _ => Err(FtmParameterError::InvalidFormatAndBandwidth),
        }
    }

    pub const fn wire(self) -> u8 {
        self as u8
    }
}

/// Parameters carried by an initial FTM Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequestParameters {
    number_of_bursts_exponent: u8,
    burst_duration: FtmBurstDuration,
    min_delta_ftm_100us: u8,
    partial_tsf_timer: Option<u16>,
    asap: bool,
    ftms_per_burst: u8,
    format_and_bandwidth: FtmFormatAndBandwidth,
    burst_period_100ms: u16,
}

impl FtmRequestParameters {
    #[expect(
        clippy::too_many_arguments,
        reason = "the constructor mirrors the finite standard element fields"
    )]
    pub const fn new(
        number_of_bursts_exponent: u8,
        burst_duration: FtmBurstDuration,
        min_delta_ftm_100us: u8,
        partial_tsf_timer: Option<u16>,
        asap: bool,
        ftms_per_burst: u8,
        format_and_bandwidth: FtmFormatAndBandwidth,
        burst_period_100ms: u16,
    ) -> Result<Self, FtmParameterError> {
        if number_of_bursts_exponent > 15 || ftms_per_burst > 31 {
            return Err(FtmParameterError::ValueOutOfRange);
        }
        if let Some(partial_tsf_timer) = partial_tsf_timer
            && partial_tsf_timer > MAX_UNAMBIGUOUS_PARTIAL_TSF
        {
            return Err(FtmParameterError::PartialTsfOutOfRange);
        }
        if number_of_bursts_exponent == 0 && burst_period_100ms != 0 {
            return Err(FtmParameterError::BurstPeriodReserved);
        }
        Ok(Self {
            number_of_bursts_exponent,
            burst_duration,
            min_delta_ftm_100us,
            partial_tsf_timer,
            asap,
            ftms_per_burst,
            format_and_bandwidth,
            burst_period_100ms,
        })
    }

    pub const fn number_of_bursts_exponent(self) -> u8 {
        self.number_of_bursts_exponent
    }

    pub const fn burst_duration(self) -> FtmBurstDuration {
        self.burst_duration
    }

    pub const fn min_delta_ftm_100us(self) -> u8 {
        self.min_delta_ftm_100us
    }

    pub const fn partial_tsf_timer(self) -> Option<u16> {
        self.partial_tsf_timer
    }

    pub const fn asap(self) -> bool {
        self.asap
    }

    /// Zero is the wire value for no preference.
    pub const fn ftms_per_burst(self) -> u8 {
        self.ftms_per_burst
    }

    pub const fn format_and_bandwidth(self) -> FtmFormatAndBandwidth {
        self.format_and_bandwidth
    }

    pub const fn burst_period_100ms(self) -> u16 {
        self.burst_period_100ms
    }

    pub fn encode_element(self) -> [u8; FTM_PARAMETERS_ELEMENT_LEN] {
        let mut element = [0_u8; FTM_PARAMETERS_ELEMENT_LEN];
        element[0] = FTM_PARAMETERS_ELEMENT_ID;
        element[1] = FTM_PARAMETERS_FIELD_LEN as u8;
        element[3] = self.number_of_bursts_exponent | (self.burst_duration.wire() << 4);
        element[4] = self.min_delta_ftm_100us;
        let partial_tsf = self.partial_tsf_timer.unwrap_or(0).to_le_bytes();
        element[5..7].copy_from_slice(&partial_tsf);
        element[7] = u8::from(self.partial_tsf_timer.is_none())
            | (u8::from(self.asap) << 2)
            | (self.ftms_per_burst << 3);
        element[8] = self.format_and_bandwidth.wire() << 2;
        element[9..11].copy_from_slice(&self.burst_period_100ms.to_le_bytes());
        element
    }

    pub fn decode_element(element: &[u8]) -> Result<Self, FtmParameterError> {
        let fields = parameter_fields(element)?;
        if fields[0] != 0 {
            return Err(FtmParameterError::ReservedStatus);
        }
        if fields[5] & 0b0000_0010 != 0 {
            return Err(FtmParameterError::RequestAsapCapableSet);
        }
        let partial_tsf = u16::from_le_bytes([fields[3], fields[4]]);
        let partial_tsf_no_preference = fields[5] & 1 != 0;
        if partial_tsf_no_preference && partial_tsf != 0 {
            return Err(FtmParameterError::ReservedBitsSet);
        }
        Self::new(
            fields[1] & 0x0f,
            FtmBurstDuration::from_wire(fields[1] >> 4)?,
            fields[2],
            if !partial_tsf_no_preference {
                Some(partial_tsf)
            } else {
                None
            },
            fields[5] & 0b0000_0100 != 0,
            fields[5] >> 3,
            FtmFormatAndBandwidth::from_wire(fields[6] >> 2)?,
            u16::from_le_bytes([fields[7], fields[8]]),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmResponseStatus {
    Success,
    Incapable,
    Failed { retry_after_seconds: u8 },
}

/// Negotiated parameters carried by the initial FTM Measurement frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmResponseParameters {
    pub status: FtmResponseStatus,
    pub number_of_bursts_exponent: u8,
    pub burst_duration: FtmBurstDuration,
    pub min_delta_ftm_100us: u8,
    pub partial_tsf_timer: u16,
    pub asap_capable: bool,
    pub asap: bool,
    pub ftms_per_burst: u8,
    pub format_and_bandwidth: FtmFormatAndBandwidth,
    pub burst_period_100ms: u16,
}

impl FtmResponseParameters {
    pub fn decode_element(element: &[u8]) -> Result<Self, FtmParameterError> {
        let fields = parameter_fields(element)?;
        let status_code = fields[0] & 0x03;
        let value = (fields[0] >> 2) & 0x1f;
        let status = match (status_code, value) {
            (1, 0) => FtmResponseStatus::Success,
            (2, 0) => FtmResponseStatus::Incapable,
            (3, retry_after_seconds) => FtmResponseStatus::Failed {
                retry_after_seconds,
            },
            (0, _) => return Err(FtmParameterError::ReservedStatus),
            _ => return Err(FtmParameterError::ReservedBitsSet),
        };
        if fields[5] & 1 != 0 {
            return Err(FtmParameterError::ReservedBitsSet);
        }
        let burst_duration = FtmBurstDuration::from_wire(fields[1] >> 4)?;
        let format_and_bandwidth = FtmFormatAndBandwidth::from_wire(fields[6] >> 2)?;
        if matches!(status, FtmResponseStatus::Success)
            && (burst_duration == FtmBurstDuration::NoPreference
                || fields[5] >> 3 == 0
                || format_and_bandwidth == FtmFormatAndBandwidth::NoPreference)
        {
            return Err(FtmParameterError::ResponseNoPreference);
        }
        let number_of_bursts_exponent = fields[1] & 0x0f;
        let burst_period_100ms = u16::from_le_bytes([fields[7], fields[8]]);
        if number_of_bursts_exponent == 0 && burst_period_100ms != 0 {
            return Err(FtmParameterError::BurstPeriodReserved);
        }
        Ok(Self {
            status,
            number_of_bursts_exponent,
            burst_duration,
            min_delta_ftm_100us: fields[2],
            partial_tsf_timer: u16::from_le_bytes([fields[3], fields[4]]),
            asap_capable: fields[5] & 0b0000_0010 != 0,
            asap: fields[5] & 0b0000_0100 != 0,
            ftms_per_burst: fields[5] >> 3,
            format_and_bandwidth,
            burst_period_100ms,
        })
    }

    pub fn encode_element(self) -> Result<[u8; FTM_PARAMETERS_ELEMENT_LEN], FtmParameterError> {
        if self.number_of_bursts_exponent > 15 || self.ftms_per_burst > 31 {
            return Err(FtmParameterError::ValueOutOfRange);
        }
        if let FtmResponseStatus::Failed {
            retry_after_seconds,
        } = self.status
            && retry_after_seconds > 31
        {
            return Err(FtmParameterError::ValueOutOfRange);
        }
        let mut element = [0_u8; FTM_PARAMETERS_ELEMENT_LEN];
        element[0] = FTM_PARAMETERS_ELEMENT_ID;
        element[1] = FTM_PARAMETERS_FIELD_LEN as u8;
        element[2] = match self.status {
            FtmResponseStatus::Success => 1,
            FtmResponseStatus::Incapable => 2,
            FtmResponseStatus::Failed {
                retry_after_seconds,
            } => 3 | (retry_after_seconds << 2),
        };
        element[3] = self.number_of_bursts_exponent | (self.burst_duration.wire() << 4);
        element[4] = self.min_delta_ftm_100us;
        element[5..7].copy_from_slice(&self.partial_tsf_timer.to_le_bytes());
        element[7] = (u8::from(self.asap_capable) << 1)
            | (u8::from(self.asap) << 2)
            | (self.ftms_per_burst << 3);
        element[8] = self.format_and_bandwidth.wire() << 2;
        element[9..11].copy_from_slice(&self.burst_period_100ms.to_le_bytes());
        Self::decode_element(&element)?;
        Ok(element)
    }
}

fn parameter_fields(element: &[u8]) -> Result<[u8; FTM_PARAMETERS_FIELD_LEN], FtmParameterError> {
    if element.first().copied() != Some(FTM_PARAMETERS_ELEMENT_ID) {
        return Err(FtmParameterError::WrongElementId);
    }
    if element.get(1).copied() != Some(FTM_PARAMETERS_FIELD_LEN as u8)
        || element.len() != FTM_PARAMETERS_ELEMENT_LEN
    {
        return Err(FtmParameterError::WrongElementLength);
    }
    let mut fields = [0_u8; FTM_PARAMETERS_FIELD_LEN];
    fields.copy_from_slice(&element[2..]);
    if fields[0] & 0x80 != 0 || fields[6] & 0x03 != 0 {
        return Err(FtmParameterError::ReservedBitsSet);
    }
    Ok(fields)
}

/// Borrowed, validated FTM Request Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequest<'a> {
    pub trigger: FtmTrigger,
    pub information_elements: &'a [u8],
    pub parameters: Option<FtmRequestParameters>,
}

impl<'a> FtmRequest<'a> {
    pub fn decode_body(body: &'a [u8]) -> Result<Self, FtmWireError> {
        validate_prefix(body, FTM_REQUEST_PUBLIC_ACTION, FTM_REQUEST_PREFIX_LEN)?;
        let trigger = FtmTrigger::from_wire(body[2])?;
        let information_elements = &body[FTM_REQUEST_PREFIX_LEN..];
        let parameters =
            find_parameters(information_elements, FtmRequestParameters::decode_element)?;
        Ok(Self {
            trigger,
            information_elements,
            parameters,
        })
    }
}

/// Encode the complete fixed-size initial request from checked parameters.
///
/// ```compile_fail,E0061
/// use oer_ieee80211_mac::ftm::{encode_initial_request, FtmRequestParameters, FtmWireError};
/// fn arbitrary_output(parameters: FtmRequestParameters, output: &mut [u8]) -> Result<usize, FtmWireError> {
///     encode_initial_request(parameters, output)
/// }
/// ```
///
/// ```
/// use oer_ieee80211_mac::ftm::{encode_initial_request, FtmRequestParameters, FTM_INITIAL_REQUEST_BODY_LEN};
/// fn complete_body(parameters: FtmRequestParameters) -> [u8; FTM_INITIAL_REQUEST_BODY_LEN] {
///     encode_initial_request(parameters)
/// }
/// ```
pub fn encode_initial_request(
    parameters: FtmRequestParameters,
) -> [u8; FTM_INITIAL_REQUEST_BODY_LEN] {
    let mut output = [0; FTM_INITIAL_REQUEST_BODY_LEN];
    let [category, action, trigger, element @ ..] = &mut output;
    *category = PUBLIC_ACTION_CATEGORY;
    *action = FTM_REQUEST_PUBLIC_ACTION;
    *trigger = FtmTrigger::StartOrContinue.wire();
    *element = parameters.encode_element();
    output
}

/// Encode a complete trigger request without a parameters element.
///
/// ```compile_fail,E0061
/// use oer_ieee80211_mac::ftm::{encode_request_trigger, FtmTrigger, FtmWireError};
/// fn arbitrary_output(trigger: FtmTrigger, output: &mut [u8]) -> Result<usize, FtmWireError> {
///     encode_request_trigger(trigger, output)
/// }
/// ```
///
/// ```
/// use oer_ieee80211_mac::ftm::{encode_request_trigger, FtmTrigger};
/// assert_eq!(encode_request_trigger(FtmTrigger::Stop), [4, 32, 0]);
/// assert_eq!(encode_request_trigger(FtmTrigger::StartOrContinue), [4, 32, 1]);
/// ```
pub const fn encode_request_trigger(trigger: FtmTrigger) -> [u8; FTM_REQUEST_PREFIX_LEN] {
    [
        PUBLIC_ACTION_CATEGORY,
        FTM_REQUEST_PUBLIC_ACTION,
        trigger.wire(),
    ]
}

/// One 48-bit timestamp in the picosecond wire domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct FtmTimestampPs(u64);

impl FtmTimestampPs {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Result<Self, FtmWireError> {
        if value <= FTM_TIMESTAMP_MASK {
            Ok(Self(value))
        } else {
            Err(FtmWireError::TimestampOutOfRange)
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    fn decode(bytes: &[u8]) -> Self {
        let mut wide = [0_u8; 8];
        wide[..6].copy_from_slice(bytes);
        Self(u64::from_le_bytes(wide))
    }

    fn encode(self, output: &mut [u8]) {
        output.copy_from_slice(&self.0.to_le_bytes()[..6]);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmTodError {
    pub max_error_exponent: u8,
    pub not_continuous: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmToaError {
    pub max_error_exponent: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmMeasurementFields {
    pub dialog_token: u8,
    pub follow_up_dialog_token: u8,
    pub tod: FtmTimestampPs,
    pub toa: FtmTimestampPs,
    pub tod_error: FtmTodError,
    pub toa_error: FtmToaError,
}

/// Borrowed, validated FTM Measurement Action body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmMeasurement<'a> {
    pub fields: FtmMeasurementFields,
    pub information_elements: &'a [u8],
    pub parameters: Option<FtmResponseParameters>,
}

impl<'a> FtmMeasurement<'a> {
    pub fn decode_body(body: &'a [u8]) -> Result<Self, FtmWireError> {
        validate_prefix(
            body,
            FTM_MEASUREMENT_PUBLIC_ACTION,
            FTM_MEASUREMENT_PREFIX_LEN,
        )?;
        let tod = FtmTimestampPs::decode(&body[4..10]);
        let toa = FtmTimestampPs::decode(&body[10..16]);
        let tod_error_wire = u16::from_le_bytes([body[16], body[17]]);
        let toa_error_wire = u16::from_le_bytes([body[18], body[19]]);
        if tod_error_wire & 0x7fe0 != 0 || toa_error_wire & 0xffe0 != 0 {
            return Err(FtmWireError::ReservedTimestampErrorBits);
        }
        let fields = FtmMeasurementFields {
            dialog_token: body[2],
            follow_up_dialog_token: body[3],
            tod,
            toa,
            tod_error: FtmTodError {
                max_error_exponent: (tod_error_wire & 0x1f) as u8,
                not_continuous: tod_error_wire & 0x8000 != 0,
            },
            toa_error: FtmToaError {
                max_error_exponent: (toa_error_wire & 0x1f) as u8,
            },
        };
        if fields.follow_up_dialog_token == 0
            && (fields.tod != FtmTimestampPs::ZERO
                || fields.toa != FtmTimestampPs::ZERO
                || tod_error_wire != 0
                || toa_error_wire != 0)
        {
            return Err(FtmWireError::ReservedTimestampFields);
        }
        let information_elements = &body[FTM_MEASUREMENT_PREFIX_LEN..];
        let parameters =
            find_parameters(information_elements, FtmResponseParameters::decode_element)?;
        Ok(Self {
            fields,
            information_elements,
            parameters,
        })
    }
}

pub fn encode_measurement(
    fields: FtmMeasurementFields,
    information_elements: &[u8],
    output: &mut [u8],
) -> Result<usize, FtmWireError> {
    find_parameters(information_elements, FtmResponseParameters::decode_element)?;
    if fields.tod_error.max_error_exponent > 31 || fields.toa_error.max_error_exponent > 31 {
        return Err(FtmWireError::ReservedTimestampErrorBits);
    }
    if fields.follow_up_dialog_token == 0
        && (fields.tod != FtmTimestampPs::ZERO
            || fields.toa != FtmTimestampPs::ZERO
            || fields.tod_error.max_error_exponent != 0
            || fields.tod_error.not_continuous
            || fields.toa_error.max_error_exponent != 0)
    {
        return Err(FtmWireError::ReservedTimestampFields);
    }
    let len = FTM_MEASUREMENT_PREFIX_LEN
        .checked_add(information_elements.len())
        .ok_or(FtmWireError::OutputTooShort)?;
    if output.len() < len {
        return Err(FtmWireError::OutputTooShort);
    }
    output[0] = PUBLIC_ACTION_CATEGORY;
    output[1] = FTM_MEASUREMENT_PUBLIC_ACTION;
    output[2] = fields.dialog_token;
    output[3] = fields.follow_up_dialog_token;
    fields.tod.encode(&mut output[4..10]);
    fields.toa.encode(&mut output[10..16]);
    let tod_error = u16::from(fields.tod_error.max_error_exponent)
        | if fields.tod_error.not_continuous {
            0x8000
        } else {
            0
        };
    output[16..18].copy_from_slice(&tod_error.to_le_bytes());
    output[18..20].copy_from_slice(&u16::from(fields.toa_error.max_error_exponent).to_le_bytes());
    output[FTM_MEASUREMENT_PREFIX_LEN..len].copy_from_slice(information_elements);
    Ok(len)
}

fn validate_prefix(body: &[u8], action: u8, minimum: usize) -> Result<(), FtmWireError> {
    if body.len() < minimum {
        return Err(FtmWireError::BodyTooShort);
    }
    if body[0] != PUBLIC_ACTION_CATEGORY {
        return Err(FtmWireError::WrongCategory);
    }
    if body[1] != action {
        return Err(FtmWireError::WrongPublicAction);
    }
    Ok(())
}

fn find_parameters<T>(
    mut bytes: &[u8],
    decode: impl Fn(&[u8]) -> Result<T, FtmParameterError>,
) -> Result<Option<T>, FtmWireError> {
    let mut found = None;
    while !bytes.is_empty() {
        if bytes.len() < 2 {
            return Err(FtmWireError::MalformedInformationElement);
        }
        let total = 2_usize
            .checked_add(usize::from(bytes[1]))
            .ok_or(FtmWireError::MalformedInformationElement)?;
        if bytes.len() < total {
            return Err(FtmWireError::MalformedInformationElement);
        }
        let element = &bytes[..total];
        if element[0] == FTM_PARAMETERS_ELEMENT_ID {
            if found.is_some() {
                return Err(FtmWireError::DuplicateParametersElement);
            }
            found = Some(decode(element)?);
        }
        bytes = &bytes[total..];
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
