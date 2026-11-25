//! Allocation-free IEEE 802.11 Fine Timing Measurement Action codecs.
//!
//! The field order and assigned values in this module follow IEEE Std
//! 802.11-2020, 9.4.2.167 and 9.6.7.1/32/33. This codec deliberately stops at
//! the Action body. A MAC owner remains responsible for the management header,
//! sequence control, FCS, protection policy and physical publication.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::management::DialogToken;

pub const PUBLIC_ACTION_CATEGORY: u8 = 4;
pub const FTM_REQUEST_PUBLIC_ACTION: u8 = 32;
pub const FTM_MEASUREMENT_PUBLIC_ACTION: u8 = 33;
pub const FTM_PARAMETERS_ELEMENT_ID: u8 = 206;
pub const FTM_PARAMETERS_FIELD_LEN: usize = 9;
pub const FTM_PARAMETERS_ELEMENT_LEN: usize = 2 + FTM_PARAMETERS_FIELD_LEN;
pub const FTM_REQUEST_PREFIX_LEN: usize = 3;
pub const FTM_INITIAL_REQUEST_BODY_LEN: usize = FTM_REQUEST_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN;
pub const FTM_MEASUREMENT_PREFIX_LEN: usize = 20;

/// The 48 bits of an FTM TOD or TOA timestamp.
///
/// SOURCE: IEEE Std 802.11-2020, 9.6.7.33 (Fine Timing Measurement frame
/// format: TOD and TOA are 48-bit fields in units of picoseconds).
pub const FTM_TIMESTAMP_MASK: u64 = (1_u64 << 48) - 1;
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

    /// The code as its four-bit Burst Duration field holds it.
    pub const fn wire(self) -> oer_portable_wire::U8<4> {
        match self {
            Self::Micros250 => oer_portable_wire::U8::constant::<2>(),
            Self::Micros500 => oer_portable_wire::U8::constant::<3>(),
            Self::Millis1 => oer_portable_wire::U8::constant::<4>(),
            Self::Millis2 => oer_portable_wire::U8::constant::<5>(),
            Self::Millis4 => oer_portable_wire::U8::constant::<6>(),
            Self::Millis8 => oer_portable_wire::U8::constant::<7>(),
            Self::Millis16 => oer_portable_wire::U8::constant::<8>(),
            Self::Millis32 => oer_portable_wire::U8::constant::<9>(),
            Self::Millis64 => oer_portable_wire::U8::constant::<10>(),
            Self::Millis128 => oer_portable_wire::U8::constant::<11>(),
            Self::NoPreference => oer_portable_wire::U8::constant::<15>(),
        }
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

    /// The code as its six-bit Format And Bandwidth field holds it.
    pub const fn wire(self) -> oer_portable_wire::U8<6> {
        match self {
            Self::NoPreference => oer_portable_wire::U8::constant::<0>(),
            Self::NonHt5Mhz => oer_portable_wire::U8::constant::<4>(),
            Self::NonHt10Mhz => oer_portable_wire::U8::constant::<6>(),
            Self::NonHt20Mhz => oer_portable_wire::U8::constant::<8>(),
            Self::HtMixed20Mhz => oer_portable_wire::U8::constant::<9>(),
            Self::Vht20Mhz => oer_portable_wire::U8::constant::<10>(),
            Self::HtMixed40Mhz => oer_portable_wire::U8::constant::<11>(),
            Self::Vht40Mhz => oer_portable_wire::U8::constant::<12>(),
            Self::Vht80Mhz => oer_portable_wire::U8::constant::<13>(),
            Self::Vht80Plus80Mhz => oer_portable_wire::U8::constant::<14>(),
            Self::Vht160MhzSeparateLos => oer_portable_wire::U8::constant::<15>(),
            Self::Vht160MhzSingleLo => oer_portable_wire::U8::constant::<16>(),
            Self::Dmg2160Mhz => oer_portable_wire::U8::constant::<31>(),
        }
    }
}

/// Parameters carried by an initial FTM Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmRequestParameters {
    number_of_bursts_exponent: oer_portable_wire::U8<4>,
    burst_duration: FtmBurstDuration,
    min_delta_ftm_100us: u8,
    partial_tsf_timer: Option<u16>,
    asap: FtmAsap,
    ftms_per_burst: oer_portable_wire::U8<5>,
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
        asap: FtmAsap,
        ftms_per_burst: u8,
        format_and_bandwidth: FtmFormatAndBandwidth,
        burst_period_100ms: u16,
    ) -> Result<Self, FtmParameterError> {
        let number_of_bursts_exponent = match oer_portable_wire::U8::new(number_of_bursts_exponent)
        {
            Some(value) => value,
            None => return Err(FtmParameterError::ValueOutOfRange),
        };
        let ftms_per_burst = match oer_portable_wire::U8::new(ftms_per_burst) {
            Some(value) => value,
            None => return Err(FtmParameterError::ValueOutOfRange),
        };
        if let Some(partial_tsf_timer) = partial_tsf_timer
            && partial_tsf_timer > MAX_UNAMBIGUOUS_PARTIAL_TSF
        {
            return Err(FtmParameterError::PartialTsfOutOfRange);
        }
        if number_of_bursts_exponent.get() == 0 && burst_period_100ms != 0 {
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
        self.number_of_bursts_exponent.get()
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

    pub const fn asap(self) -> FtmAsap {
        self.asap
    }

    /// Zero is the wire value for no preference.
    pub const fn ftms_per_burst(self) -> u8 {
        self.ftms_per_burst.get()
    }

    pub const fn format_and_bandwidth(self) -> FtmFormatAndBandwidth {
        self.format_and_bandwidth
    }

    pub const fn burst_period_100ms(self) -> u16 {
        self.burst_period_100ms
    }

    pub fn encode_element(self) -> [u8; FTM_PARAMETERS_ELEMENT_LEN] {
        // A request's Status Indication and Value are zero (9.4.2.167).
        let fields = FtmParameterFields::from_bytes([0; 9])
            .with_bursts(self.number_of_bursts_exponent)
            .with_duration(self.burst_duration.wire())
            .with_partial_tsf(self.partial_tsf_timer.unwrap_or(0))
            .with_no_partial_tsf(self.partial_tsf_timer.is_none())
            .with_asap(self.asap.bit())
            .with_ftms(self.ftms_per_burst)
            .with_bandwidth(self.format_and_bandwidth.wire())
            .with_burst_period(self.burst_period_100ms)
            .with_min_delta(self.min_delta_ftm_100us);
        parameters_element(fields)
    }

    pub fn decode_element(element: &[u8]) -> Result<Self, FtmParameterError> {
        let fields = parameter_fields(element)?;
        let parsed = FtmParameterFields::from_bytes(fields);
        if parsed.status() != 0 || parsed.retry() != 0 {
            return Err(FtmParameterError::ReservedStatus);
        }
        if FtmParameterFields::from_bytes(fields).asap_capable() {
            return Err(FtmParameterError::RequestAsapCapableSet);
        }
        let partial_tsf = parsed.partial_tsf();
        let partial_tsf_no_preference = FtmParameterFields::from_bytes(fields).no_partial_tsf();
        if partial_tsf_no_preference && partial_tsf != 0 {
            return Err(FtmParameterError::ReservedBitsSet);
        }
        Self::new(
            FtmParameterFields::from_bytes(fields).bursts().get(),
            FtmBurstDuration::from_wire(FtmParameterFields::from_bytes(fields).duration().get())?,
            parsed.min_delta(),
            if !partial_tsf_no_preference {
                Some(partial_tsf)
            } else {
                None
            },
            if FtmParameterFields::from_bytes(fields).asap() {
                FtmAsap::AsSoonAsPossible
            } else {
                FtmAsap::AtPartialTsf
            },
            FtmParameterFields::from_bytes(fields).ftms().get(),
            FtmFormatAndBandwidth::from_wire(
                FtmParameterFields::from_bytes(fields).bandwidth().get(),
            )?,
            parsed.burst_period(),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmResponseStatus {
    Success,
    Incapable,
    Failed { retry_after_seconds: u8 },
}

/// The ASAP Capable subfield: whether the responding STA can start a burst
/// as soon as possible (IEEE Std 802.11-2020, 9.4.2.167).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmAsapCapability {
    Capable,
    Incapable,
}

impl FtmAsapCapability {
    const fn bit(self) -> bool {
        matches!(self, Self::Capable)
    }
}

/// The ASAP subfield: whether the first burst starts as soon as possible or
/// at the partial TSF timer (IEEE Std 802.11-2020, 9.4.2.167).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FtmAsap {
    AsSoonAsPossible,
    AtPartialTsf,
}

impl FtmAsap {
    const fn bit(self) -> bool {
        matches!(self, Self::AsSoonAsPossible)
    }
}

/// Negotiated parameters carried by the initial FTM Measurement frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FtmResponseParameters {
    pub status: FtmResponseStatus,
    pub number_of_bursts_exponent: u8,
    pub burst_duration: FtmBurstDuration,
    pub min_delta_ftm_100us: u8,
    pub partial_tsf_timer: u16,
    pub asap_capable: FtmAsapCapability,
    pub asap: FtmAsap,
    pub ftms_per_burst: u8,
    pub format_and_bandwidth: FtmFormatAndBandwidth,
    pub burst_period_100ms: u16,
}

impl FtmResponseParameters {
    pub fn decode_element(element: &[u8]) -> Result<Self, FtmParameterError> {
        let fields = parameter_fields(element)?;
        let status_code = FtmParameterFields::from_bytes(fields).status();
        let value = FtmParameterFields::from_bytes(fields).retry();
        let status = match (status_code, value) {
            (1, 0) => FtmResponseStatus::Success,
            (2, 0) => FtmResponseStatus::Incapable,
            (3, retry_after_seconds) => FtmResponseStatus::Failed {
                retry_after_seconds,
            },
            (0, _) => return Err(FtmParameterError::ReservedStatus),
            _ => return Err(FtmParameterError::ReservedBitsSet),
        };
        if FtmParameterFields::from_bytes(fields).no_partial_tsf() {
            return Err(FtmParameterError::ReservedBitsSet);
        }
        let burst_duration =
            FtmBurstDuration::from_wire(FtmParameterFields::from_bytes(fields).duration().get())?;
        let format_and_bandwidth = FtmFormatAndBandwidth::from_wire(
            FtmParameterFields::from_bytes(fields).bandwidth().get(),
        )?;
        if matches!(status, FtmResponseStatus::Success)
            && (burst_duration == FtmBurstDuration::NoPreference
                || FtmParameterFields::from_bytes(fields).ftms().get() == 0
                || format_and_bandwidth == FtmFormatAndBandwidth::NoPreference)
        {
            return Err(FtmParameterError::ResponseNoPreference);
        }
        let number_of_bursts_exponent = FtmParameterFields::from_bytes(fields).bursts().get();
        let parsed = FtmParameterFields::from_bytes(fields);
        let burst_period_100ms = parsed.burst_period();
        if number_of_bursts_exponent == 0 && burst_period_100ms != 0 {
            return Err(FtmParameterError::BurstPeriodReserved);
        }
        Ok(Self {
            status,
            number_of_bursts_exponent,
            burst_duration,
            min_delta_ftm_100us: parsed.min_delta(),
            partial_tsf_timer: parsed.partial_tsf(),
            asap_capable: if FtmParameterFields::from_bytes(fields).asap_capable() {
                FtmAsapCapability::Capable
            } else {
                FtmAsapCapability::Incapable
            },
            asap: if FtmParameterFields::from_bytes(fields).asap() {
                FtmAsap::AsSoonAsPossible
            } else {
                FtmAsap::AtPartialTsf
            },
            ftms_per_burst: FtmParameterFields::from_bytes(fields).ftms().get(),
            format_and_bandwidth,
            burst_period_100ms,
        })
    }

    pub fn encode_element(self) -> Result<[u8; FTM_PARAMETERS_ELEMENT_LEN], FtmParameterError> {
        let bursts = oer_portable_wire::U8::new(self.number_of_bursts_exponent)
            .ok_or(FtmParameterError::ValueOutOfRange)?;
        let ftms = oer_portable_wire::U8::new(self.ftms_per_burst)
            .ok_or(FtmParameterError::ValueOutOfRange)?;
        let (status, retry) = match self.status {
            FtmResponseStatus::Success => (1, 0),
            FtmResponseStatus::Incapable => (2, 0),
            FtmResponseStatus::Failed {
                retry_after_seconds,
            } => (3, retry_after_seconds),
        };
        let fields = FtmParameterFields::from_bytes([0; 9])
            .with_status(status)
            .and_then(|fields| fields.with_retry(retry))
            .map_err(|_| FtmParameterError::ValueOutOfRange)?
            .with_min_delta(self.min_delta_ftm_100us)
            .with_bursts(bursts)
            .with_duration(self.burst_duration.wire())
            .with_partial_tsf(self.partial_tsf_timer)
            .with_asap_capable(self.asap_capable.bit())
            .with_asap(self.asap.bit())
            .with_ftms(ftms)
            .with_bandwidth(self.format_and_bandwidth.wire())
            .with_burst_period(self.burst_period_100ms);
        let element = parameters_element(fields);
        Self::decode_element(&element)?;
        Ok(element)
    }
}

fn parameter_fields(element: &[u8]) -> Result<[u8; FTM_PARAMETERS_FIELD_LEN], FtmParameterError> {
    if element.first().copied() != Some(FTM_PARAMETERS_ELEMENT_ID) {
        return Err(FtmParameterError::WrongElementId);
    }
    if element.get(1).copied()
        != Some(const { crate::encoding::octet::<FTM_PARAMETERS_FIELD_LEN>() })
        || element.len() != FTM_PARAMETERS_ELEMENT_LEN
    {
        return Err(FtmParameterError::WrongElementLength);
    }
    let fields: [u8; FTM_PARAMETERS_FIELD_LEN] = element
        .get(2..)
        .and_then(|fields| fields.try_into().ok())
        .ok_or(FtmParameterError::WrongElementLength)?;
    if FtmParameterFields::from_bytes(fields).status_reserved()
        || FtmParameterFields::from_bytes(fields).bandwidth_reserved() != 0
    {
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
        let (&[_, _, trigger], information_elements) =
            validate_prefix::<FTM_REQUEST_PREFIX_LEN>(body, FTM_REQUEST_PUBLIC_ACTION)?;
        let trigger = FtmTrigger::from_wire(trigger)?;
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
/// use oer_protocols_ieee80211_wire::ftm::{encode_initial_request, FtmRequestParameters, FtmWireError};
/// fn arbitrary_output(parameters: FtmRequestParameters, output: &mut [u8]) -> Result<usize, FtmWireError> {
///     encode_initial_request(parameters, output)
/// }
/// ```
///
/// ```
/// use oer_protocols_ieee80211_wire::ftm::{encode_initial_request, FtmRequestParameters, FTM_INITIAL_REQUEST_BODY_LEN};
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
/// use oer_protocols_ieee80211_wire::ftm::{encode_request_trigger, FtmTrigger, FtmWireError};
/// fn arbitrary_output(trigger: FtmTrigger, output: &mut [u8]) -> Result<usize, FtmWireError> {
///     encode_request_trigger(trigger, output)
/// }
/// ```
///
/// ```
/// use oer_protocols_ieee80211_wire::ftm::{encode_request_trigger, FtmTrigger};
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
pub struct FtmTimestampPs(oer_portable_wire::U64<48>);

impl FtmTimestampPs {
    pub const ZERO: Self = Self(oer_portable_wire::U64::ZERO);

    pub const fn new(value: u64) -> Result<Self, FtmWireError> {
        match oer_portable_wire::U64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(FtmWireError::TimestampOutOfRange),
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }

    fn decode(octets: [u8; 6]) -> Self {
        Self(FtmTimestampField::from_bytes(octets).picoseconds())
    }

    fn encode(self) -> [u8; 6] {
        FtmTimestampField::from_bytes([0; 6])
            .with_picoseconds(self.0)
            .to_bytes()
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
    pub dialog_token: DialogToken,
    pub follow_up_dialog_token: DialogToken,
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
        let (
            &[
                _,
                _,
                dialog_token,
                follow_up_dialog_token,
                d0,
                d1,
                d2,
                d3,
                d4,
                d5,
                a0,
                a1,
                a2,
                a3,
                a4,
                a5,
                e0,
                e1,
                g0,
                g1,
            ],
            information_elements,
        ) = validate_prefix::<FTM_MEASUREMENT_PREFIX_LEN>(body, FTM_MEASUREMENT_PUBLIC_ACTION)?;
        let tod = FtmTimestampPs::decode([d0, d1, d2, d3, d4, d5]);
        let toa = FtmTimestampPs::decode([a0, a1, a2, a3, a4, a5]);
        let tod_error = FtmTimestampErrorFields::from_bytes([e0, e1]);
        let toa_error = FtmTimestampErrorFields::from_bytes([g0, g1]);
        let no_error = FtmTimestampErrorFields::from_bytes([0; 2]);
        if tod_error.reserved() != 0 || toa_error.reserved() != 0 || toa_error.not_continuous() {
            return Err(FtmWireError::ReservedTimestampErrorBits);
        }
        let fields = FtmMeasurementFields {
            dialog_token: DialogToken::new(dialog_token),
            follow_up_dialog_token: DialogToken::new(follow_up_dialog_token),
            tod,
            toa,
            tod_error: FtmTodError {
                max_error_exponent: tod_error.exponent(),
                not_continuous: tod_error.not_continuous(),
            },
            toa_error: FtmToaError {
                max_error_exponent: toa_error.exponent(),
            },
        };
        if fields.follow_up_dialog_token == DialogToken::ZERO
            && (fields.tod != FtmTimestampPs::ZERO
                || fields.toa != FtmTimestampPs::ZERO
                || tod_error != no_error
                || toa_error != no_error)
        {
            return Err(FtmWireError::ReservedTimestampFields);
        }
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
    if fields.follow_up_dialog_token == DialogToken::ZERO
        && (fields.tod != FtmTimestampPs::ZERO
            || fields.toa != FtmTimestampPs::ZERO
            || fields.tod_error.max_error_exponent != 0
            || fields.tod_error.not_continuous
            || fields.toa_error.max_error_exponent != 0)
    {
        return Err(FtmWireError::ReservedTimestampFields);
    }
    let (prefix, rest) = output
        .split_first_chunk_mut::<FTM_MEASUREMENT_PREFIX_LEN>()
        .ok_or(FtmWireError::OutputTooShort)?;
    let elements = rest
        .get_mut(..information_elements.len())
        .ok_or(FtmWireError::OutputTooShort)?;
    let [d0, d1, d2, d3, d4, d5] = fields.tod.encode();
    let [a0, a1, a2, a3, a4, a5] = fields.toa.encode();
    let [e0, e1] = FtmTimestampErrorFields::from_bytes([0; 2])
        .with_not_continuous(fields.tod_error.not_continuous)
        .with_exponent(fields.tod_error.max_error_exponent)
        .map_err(|_| FtmWireError::ReservedTimestampErrorBits)?
        .to_bytes();
    let [g0, g1] = FtmTimestampErrorFields::from_bytes([0; 2])
        .with_exponent(fields.toa_error.max_error_exponent)
        .map_err(|_| FtmWireError::ReservedTimestampErrorBits)?
        .to_bytes();
    *prefix = [
        PUBLIC_ACTION_CATEGORY,
        FTM_MEASUREMENT_PUBLIC_ACTION,
        fields.dialog_token.value(),
        fields.follow_up_dialog_token.value(),
        d0,
        d1,
        d2,
        d3,
        d4,
        d5,
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        e0,
        e1,
        g0,
        g1,
    ];
    oer_portable_wire::Writer::new(elements)
        .write(information_elements)
        .map_err(|_| FtmWireError::OutputTooShort)?;
    FTM_MEASUREMENT_PREFIX_LEN
        .checked_add(information_elements.len())
        .ok_or(FtmWireError::OutputTooShort)
}

/// Split the fixed `N`-byte prefix off `body` after checking its Public Action
/// category and action code.
fn validate_prefix<const N: usize>(
    body: &[u8],
    action: u8,
) -> Result<(&[u8; N], &[u8]), FtmWireError> {
    let mut reader = oer_portable_wire::Reader::new(body);
    let prefix = reader
        .array::<N>()
        .map_err(|_| FtmWireError::BodyTooShort)?;
    let rest = reader.remaining();
    let (&category, tail) = prefix.split_first().ok_or(FtmWireError::BodyTooShort)?;
    let &public_action = tail.first().ok_or(FtmWireError::BodyTooShort)?;
    if category != PUBLIC_ACTION_CATEGORY {
        return Err(FtmWireError::WrongCategory);
    }
    if public_action != action {
        return Err(FtmWireError::WrongPublicAction);
    }
    Ok((prefix, rest))
}

fn find_parameters<T>(
    mut bytes: &[u8],
    decode: impl Fn(&[u8]) -> Result<T, FtmParameterError>,
) -> Result<Option<T>, FtmWireError> {
    let mut found = None;
    while !bytes.is_empty() {
        let mut reader = oer_portable_wire::Reader::new(bytes);
        let &[id, length] = reader
            .array::<2>()
            .map_err(|_| FtmWireError::MalformedInformationElement)?;
        let total = 2_usize
            .checked_add(usize::from(length))
            .ok_or(FtmWireError::MalformedInformationElement)?;
        let mut whole = oer_portable_wire::Reader::new(bytes);
        let element = whole
            .take(total)
            .map_err(|_| FtmWireError::MalformedInformationElement)?;
        let rest = whole.remaining();
        if id == FTM_PARAMETERS_ELEMENT_ID {
            if found.is_some() {
                return Err(FtmWireError::DuplicateParametersElement);
            }
            found = Some(decode(element)?);
        }
        bytes = rest;
    }
    Ok(found)
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.167 (FTM Parameters element).
    pub struct FtmParameterFields[9] (little) {
        pub status, with_status: u8 = 0..2;
        pub retry, with_retry: u8 = 2..7;
        pub status_reserved, with_status_reserved: bool = 7..8;
        pub bursts, with_bursts: (u8) = 8..12;
        pub duration, with_duration: (u8) = 12..16;
        pub min_delta, with_min_delta: {u8} = 16..24;
        pub partial_tsf, with_partial_tsf: {u16} = 24..40;
        pub no_partial_tsf, with_no_partial_tsf: bool = 40..41;
        pub asap_capable, with_asap_capable: bool = 41..42;
        pub asap, with_asap: bool = 42..43;
        pub ftms, with_ftms: (u8) = 43..48;
        pub bandwidth_reserved, with_bandwidth_reserved: u8 = 48..50;
        pub bandwidth, with_bandwidth: (u8) = 50..56;
        pub burst_period, with_burst_period: {u16} = 56..72;
    }
}
oer_portable_wire::bitfield! {
    /// A Time of Departure or Time of Arrival field: 48 bits, in
    /// picoseconds.
    /// SOURCE: IEEE Std 802.11-2020, 9.6.7.33 (Fine Timing Measurement
    /// frame format).
    pub struct FtmTimestampField[6] (little) {
        pub picoseconds, with_picoseconds: (u64) = 0..48;
    }
}

/// The FTM Parameters element holding `fields`.
const fn parameters_element(fields: FtmParameterFields) -> [u8; FTM_PARAMETERS_ELEMENT_LEN] {
    let [f0, f1, f2, f3, f4, f5, f6, f7, f8] = fields.to_bytes();
    [
        FTM_PARAMETERS_ELEMENT_ID,
        const { crate::encoding::octet::<FTM_PARAMETERS_FIELD_LEN>() },
        f0,
        f1,
        f2,
        f3,
        f4,
        f5,
        f6,
        f7,
        f8,
    ]
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.6.7.33 (Fine Timing Measurement frame).
    pub struct FtmTimestampErrorFields[2] (little) {
        pub exponent, with_exponent: u8 = 0..5;
        pub reserved, with_reserved: u16 = 5..15;
        pub not_continuous, with_not_continuous: bool = 15..16;
    }
}

#[cfg(test)]
mod tests;
