//! Allocation-free IEEE 802.11 individual Target Wake Time action codec.
//!
//! This module deliberately implements only HE individual TWT negotiation
//! type zero. Wake-TBTT, broadcast TWT, NDP paging, MLO extensions and
//! restricted TWT have different variable layouts and are rejected rather
//! than interpreted as this fixed format.
//!
//! The field layout follows IEEE Std 802.11-2020, 9.4.2.199 and the public
//! definitions in Linux v6.1 `include/linux/ieee80211.h`: category 22,
//! actions 6/7, element 216, a 15-octet individual TWT element body and the
//! Request Type bit allocation. The teardown layout follows IEEE Std
//! 802.11ax-2021 Figure 9-965: flow ID in bits 0..=2, negotiation type in
//! bits 5..=6 and Teardown All TWT in bit 7.

pub const TWT_ACTION_CATEGORY: u8 = 22;
pub const TWT_SETUP_ACTION: u8 = 6;
pub const TWT_TEARDOWN_ACTION: u8 = 7;
pub const TWT_ELEMENT_ID: u8 = 216;
pub const INDIVIDUAL_TWT_ELEMENT_BODY_LEN: u8 = 15;
pub const INDIVIDUAL_TWT_SETUP_BODY_LEN: usize = 20;
pub const INDIVIDUAL_TWT_TEARDOWN_BODY_LEN: usize = 3;
pub const INDIVIDUAL_TWT_FLOW_CAPACITY: usize = 8;

const CONTROL_NDP_PAGING: u8 = 1 << 0;
const CONTROL_RESPONDER_PM: u8 = 1 << 1;
const CONTROL_NEGOTIATION_TYPE: u8 = 0b11 << 2;
const CONTROL_INFORMATION_DISABLED: u8 = 1 << 4;
const CONTROL_WAKE_DURATION_UNIT: u8 = 1 << 5;
const CONTROL_RESERVED: u8 = 0b11 << 6;

const REQUEST_REQUESTING_STA: u16 = 1 << 0;
const REQUEST_SETUP_COMMAND: u16 = 0b111 << 1;
const REQUEST_TRIGGER: u16 = 1 << 4;
const REQUEST_IMPLICIT: u16 = 1 << 5;
const REQUEST_FLOW_TYPE: u16 = 1 << 6;
const REQUEST_FLOW_ID: u16 = 0b111 << 7;
const REQUEST_WAKE_INTERVAL_EXPONENT: u16 = 0b1_1111 << 10;
const REQUEST_PROTECTION: u16 = 1 << 15;

const TEARDOWN_FLOW_ID: u8 = 0b111;
const TEARDOWN_RESERVED: u8 = 0b11 << 3;
const TEARDOWN_NEGOTIATION_TYPE: u8 = 0b11 << 5;
const TEARDOWN_ALL: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TwtWireError {
    OutputTooSmall { required: usize, available: usize },
    InvalidLength { expected: usize, actual: usize },
    InvalidElement,
    UnsupportedAction(u8),
    UnsupportedControl(u8),
    UnsupportedNegotiationType(u8),
    InvalidFlowId(u8),
    InvalidSetupCommand(u8),
    UnsupportedSetupCommand(IndividualTwtSetupCommand),
    RequestCommandFromResponder,
    ResponseCommandFromRequester,
    InvalidWakeIntervalExponent(u8),
    ZeroWakeIntervalMantissa,
    ZeroWakeDuration,
    WakeDurationExceedsInterval,
    UnsupportedTwtChannel(u8),
    ReservedTeardownBits(u8),
    NonzeroFlowForTeardownAll(u8),
}

/// Three-bit individual TWT flow identifier.
/// Its closed value can select a cell of an exact eight-flow array without
/// turning the previously checked identifier back into an unchecked index.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct IndividualTwtFlowId(FlowNumber);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
enum FlowNumber {
    Flow0 = 0,
    Flow1 = 1,
    Flow2 = 2,
    Flow3 = 3,
    Flow4 = 4,
    Flow5 = 5,
    Flow6 = 6,
    Flow7 = 7,
}

impl IndividualTwtFlowId {
    /// Every representable flow, in wire order.
    pub const ALL: [Self; 8] = [
        Self(FlowNumber::Flow0),
        Self(FlowNumber::Flow1),
        Self(FlowNumber::Flow2),
        Self(FlowNumber::Flow3),
        Self(FlowNumber::Flow4),
        Self(FlowNumber::Flow5),
        Self(FlowNumber::Flow6),
        Self(FlowNumber::Flow7),
    ];
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self(FlowNumber::Flow0)),
            1 => Some(Self(FlowNumber::Flow1)),
            2 => Some(Self(FlowNumber::Flow2)),
            3 => Some(Self(FlowNumber::Flow3)),
            4 => Some(Self(FlowNumber::Flow4)),
            5 => Some(Self(FlowNumber::Flow5)),
            6 => Some(Self(FlowNumber::Flow6)),
            7 => Some(Self(FlowNumber::Flow7)),
            _ => None,
        }
    }
    pub const fn get(self) -> u8 {
        self.0 as u8
    }
    pub const fn index(self) -> usize {
        self.get() as usize
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select<T>(self, entries: &[T; 8]) -> &T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            FlowNumber::Flow0 => a,
            FlowNumber::Flow1 => b,
            FlowNumber::Flow2 => c,
            FlowNumber::Flow3 => d,
            FlowNumber::Flow4 => e,
            FlowNumber::Flow5 => f,
            FlowNumber::Flow6 => g,
            FlowNumber::Flow7 => h,
        }
    }
    /// Select a cell of the exact eight-entry array by the closed identifier.
    pub const fn select_mut<T>(self, entries: &mut [T; 8]) -> &mut T {
        let [a, b, c, d, e, f, g, h] = entries;
        match self.0 {
            FlowNumber::Flow0 => a,
            FlowNumber::Flow1 => b,
            FlowNumber::Flow2 => c,
            FlowNumber::Flow3 => d,
            FlowNumber::Flow4 => e,
            FlowNumber::Flow5 => f,
            FlowNumber::Flow6 => g,
            FlowNumber::Flow7 => h,
        }
    }
}

/// TWT Setup Command values from the two-byte Request Type field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum IndividualTwtSetupCommand {
    Request = 0,
    Suggest = 1,
    Demand = 2,
    Grouping = 3,
    Accept = 4,
    Alternate = 5,
    Dictate = 6,
    Reject = 7,
}

impl IndividualTwtSetupCommand {
    const fn parse(value: u8) -> Result<Self, TwtWireError> {
        match value {
            0 => Ok(Self::Request),
            1 => Ok(Self::Suggest),
            2 => Ok(Self::Demand),
            3 => Ok(Self::Grouping),
            4 => Ok(Self::Accept),
            5 => Ok(Self::Alternate),
            6 => Ok(Self::Dictate),
            7 => Ok(Self::Reject),
            _ => Err(TwtWireError::InvalidSetupCommand(value)),
        }
    }

    pub const fn is_requester_command(self) -> bool {
        matches!(self, Self::Request | Self::Suggest | Self::Demand)
    }

    pub const fn is_responder_command(self) -> bool {
        matches!(
            self,
            Self::Accept | Self::Alternate | Self::Dictate | Self::Reject
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtFlowType {
    Announced,
    Unannounced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TwtWakeDurationUnit {
    Micros256,
    Tu1024,
}

impl TwtWakeDurationUnit {
    pub const fn micros(self) -> u32 {
        match self {
            Self::Micros256 => 256,
            Self::Tu1024 => 1_024,
        }
    }
}

/// Supported fixed portion of the individual TWT Control field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtControl {
    pub responder_power_save: bool,
    pub information_frames_disabled: bool,
    pub wake_duration_unit: TwtWakeDurationUnit,
}

impl IndividualTwtControl {
    pub const REQUEST: Self = Self {
        responder_power_save: false,
        information_frames_disabled: false,
        wake_duration_unit: TwtWakeDurationUnit::Micros256,
    };

    const fn encode(self) -> u8 {
        ((self.responder_power_save as u8) * CONTROL_RESPONDER_PM)
            | ((self.information_frames_disabled as u8) * CONTROL_INFORMATION_DISABLED)
            | match self.wake_duration_unit {
                TwtWakeDurationUnit::Micros256 => 0,
                TwtWakeDurationUnit::Tu1024 => CONTROL_WAKE_DURATION_UNIT,
            }
    }

    const fn parse(value: u8) -> Result<Self, TwtWireError> {
        let negotiation_type = (value & CONTROL_NEGOTIATION_TYPE) >> 2;
        if negotiation_type != 0 {
            return Err(TwtWireError::UnsupportedNegotiationType(negotiation_type));
        }
        if value & (CONTROL_NDP_PAGING | CONTROL_RESERVED) != 0 {
            return Err(TwtWireError::UnsupportedControl(value));
        }
        Ok(Self {
            responder_power_save: value & CONTROL_RESPONDER_PM != 0,
            information_frames_disabled: value & CONTROL_INFORMATION_DISABLED != 0,
            wake_duration_unit: if value & CONTROL_WAKE_DURATION_UNIT == 0 {
                TwtWakeDurationUnit::Micros256
            } else {
                TwtWakeDurationUnit::Tu1024
            },
        })
    }
}

/// One fixed individual TWT parameter set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtParameterSet {
    pub requesting_sta: bool,
    pub setup_command: IndividualTwtSetupCommand,
    pub trigger: bool,
    pub implicit: bool,
    pub flow_type: IndividualTwtFlowType,
    pub flow_id: IndividualTwtFlowId,
    pub wake_interval_exponent: u8,
    pub protection: bool,
    pub target_wake_time_tsf: u64,
    pub nominal_minimum_wake_duration: u8,
    pub wake_interval_mantissa: u16,
    pub twt_channel: u8,
}

impl IndividualTwtParameterSet {
    pub const fn wake_interval_micros(self) -> Result<u64, TwtWireError> {
        if self.wake_interval_exponent > 31 {
            return Err(TwtWireError::InvalidWakeIntervalExponent(
                self.wake_interval_exponent,
            ));
        }
        if self.wake_interval_mantissa == 0 {
            return Err(TwtWireError::ZeroWakeIntervalMantissa);
        }
        Ok((self.wake_interval_mantissa as u64) << self.wake_interval_exponent)
    }

    pub const fn wake_duration_micros(
        self,
        control: IndividualTwtControl,
    ) -> Result<u32, TwtWireError> {
        if self.nominal_minimum_wake_duration == 0 {
            return Err(TwtWireError::ZeroWakeDuration);
        }
        Ok(self.nominal_minimum_wake_duration as u32 * control.wake_duration_unit.micros())
    }

    pub fn validate(self, control: IndividualTwtControl) -> Result<Self, TwtWireError> {
        if matches!(self.setup_command, IndividualTwtSetupCommand::Grouping) {
            return Err(TwtWireError::UnsupportedSetupCommand(
                IndividualTwtSetupCommand::Grouping,
            ));
        }
        if self.requesting_sta && !self.setup_command.is_requester_command() {
            return Err(TwtWireError::ResponseCommandFromRequester);
        }
        if !self.requesting_sta && !self.setup_command.is_responder_command() {
            return Err(TwtWireError::RequestCommandFromResponder);
        }
        if self.twt_channel != 0 {
            return Err(TwtWireError::UnsupportedTwtChannel(self.twt_channel));
        }
        if self.setup_command != IndividualTwtSetupCommand::Reject {
            let interval = self.wake_interval_micros()?;
            let duration = self.wake_duration_micros(control)?;
            if duration as u64 > interval {
                return Err(TwtWireError::WakeDurationExceedsInterval);
            }
        }
        Ok(self)
    }

    const fn encode_request_type(self) -> u16 {
        ((self.requesting_sta as u16) * REQUEST_REQUESTING_STA)
            | ((self.setup_command as u16) << 1)
            | ((self.trigger as u16) * REQUEST_TRIGGER)
            | ((self.implicit as u16) * REQUEST_IMPLICIT)
            | match self.flow_type {
                IndividualTwtFlowType::Announced => 0,
                IndividualTwtFlowType::Unannounced => REQUEST_FLOW_TYPE,
            }
            | ((self.flow_id.get() as u16) << 7)
            | ((self.wake_interval_exponent as u16) << 10)
            | ((self.protection as u16) * REQUEST_PROTECTION)
    }

    fn parse(request_type: u16, bytes: &[u8]) -> Result<Self, TwtWireError> {
        let flow_value = ((request_type & REQUEST_FLOW_ID) >> 7) as u8;
        let flow_id =
            IndividualTwtFlowId::new(flow_value).ok_or(TwtWireError::InvalidFlowId(flow_value))?;
        let setup_value = ((request_type & REQUEST_SETUP_COMMAND) >> 1) as u8;
        Ok(Self {
            requesting_sta: request_type & REQUEST_REQUESTING_STA != 0,
            setup_command: IndividualTwtSetupCommand::parse(setup_value)?,
            trigger: request_type & REQUEST_TRIGGER != 0,
            implicit: request_type & REQUEST_IMPLICIT != 0,
            flow_type: if request_type & REQUEST_FLOW_TYPE == 0 {
                IndividualTwtFlowType::Announced
            } else {
                IndividualTwtFlowType::Unannounced
            },
            flow_id,
            wake_interval_exponent: ((request_type & REQUEST_WAKE_INTERVAL_EXPONENT) >> 10) as u8,
            protection: request_type & REQUEST_PROTECTION != 0,
            target_wake_time_tsf: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            nominal_minimum_wake_duration: bytes[8],
            wake_interval_mantissa: u16::from_le_bytes([bytes[9], bytes[10]]),
            twt_channel: bytes[11],
        })
    }
}

/// Complete body of one individual TWT Setup Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtSetup {
    pub dialog_token: u8,
    pub control: IndividualTwtControl,
    pub parameters: IndividualTwtParameterSet,
}

impl IndividualTwtSetup {
    pub fn validate(self) -> Result<Self, TwtWireError> {
        match self.parameters.validate(self.control) {
            Ok(_) => Ok(self),
            Err(error) => Err(error),
        }
    }

    /// Encode only after all inputs and output capacity have been validated.
    /// An error leaves `output` unchanged.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, TwtWireError> {
        self.validate()?;
        if output.len() < INDIVIDUAL_TWT_SETUP_BODY_LEN {
            return Err(TwtWireError::OutputTooSmall {
                required: INDIVIDUAL_TWT_SETUP_BODY_LEN,
                available: output.len(),
            });
        }

        let mut body = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        body[0] = TWT_ACTION_CATEGORY;
        body[1] = TWT_SETUP_ACTION;
        body[2] = self.dialog_token;
        body[3] = TWT_ELEMENT_ID;
        body[4] = INDIVIDUAL_TWT_ELEMENT_BODY_LEN;
        body[5] = self.control.encode();
        body[6..8].copy_from_slice(&self.parameters.encode_request_type().to_le_bytes());
        body[8..16].copy_from_slice(&self.parameters.target_wake_time_tsf.to_le_bytes());
        body[16] = self.parameters.nominal_minimum_wake_duration;
        body[17..19].copy_from_slice(&self.parameters.wake_interval_mantissa.to_le_bytes());
        body[19] = self.parameters.twt_channel;
        output[..INDIVIDUAL_TWT_SETUP_BODY_LEN].copy_from_slice(&body);
        Ok(INDIVIDUAL_TWT_SETUP_BODY_LEN)
    }

    pub fn encode_body(self) -> Result<[u8; INDIVIDUAL_TWT_SETUP_BODY_LEN], TwtWireError> {
        let mut body = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        self.encode(&mut body)?;
        Ok(body)
    }

    pub fn parse(body: &[u8]) -> Result<Self, TwtWireError> {
        if body.len() != INDIVIDUAL_TWT_SETUP_BODY_LEN {
            return Err(TwtWireError::InvalidLength {
                expected: INDIVIDUAL_TWT_SETUP_BODY_LEN,
                actual: body.len(),
            });
        }
        if body[0] != TWT_ACTION_CATEGORY
            || body[1] != TWT_SETUP_ACTION
            || body[3] != TWT_ELEMENT_ID
            || body[4] != INDIVIDUAL_TWT_ELEMENT_BODY_LEN
        {
            return Err(TwtWireError::InvalidElement);
        }
        let control = IndividualTwtControl::parse(body[5])?;
        let request_type = u16::from_le_bytes([body[6], body[7]]);
        let parameters = IndividualTwtParameterSet::parse(request_type, &body[8..20])?;
        Self {
            dialog_token: body[2],
            control,
            parameters,
        }
        .validate()
    }
}

/// Complete body of an individual TWT Teardown Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtTeardown {
    pub flow_id: IndividualTwtFlowId,
    pub all_flows: bool,
}

impl IndividualTwtTeardown {
    pub const fn one(flow_id: IndividualTwtFlowId) -> Self {
        Self {
            flow_id,
            all_flows: false,
        }
    }

    pub const fn all() -> Self {
        Self {
            flow_id: IndividualTwtFlowId(FlowNumber::Flow0),
            all_flows: true,
        }
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, TwtWireError> {
        if self.all_flows && self.flow_id.get() != 0 {
            return Err(TwtWireError::NonzeroFlowForTeardownAll(self.flow_id.get()));
        }
        if output.len() < INDIVIDUAL_TWT_TEARDOWN_BODY_LEN {
            return Err(TwtWireError::OutputTooSmall {
                required: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
                available: output.len(),
            });
        }
        let body = [
            TWT_ACTION_CATEGORY,
            TWT_TEARDOWN_ACTION,
            self.flow_id.get() | ((self.all_flows as u8) * TEARDOWN_ALL),
        ];
        output[..INDIVIDUAL_TWT_TEARDOWN_BODY_LEN].copy_from_slice(&body);
        Ok(INDIVIDUAL_TWT_TEARDOWN_BODY_LEN)
    }

    pub fn encode_body(self) -> Result<[u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN], TwtWireError> {
        let mut body = [0_u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN];
        self.encode(&mut body)?;
        Ok(body)
    }

    pub fn parse(body: &[u8]) -> Result<Self, TwtWireError> {
        if body.len() != INDIVIDUAL_TWT_TEARDOWN_BODY_LEN {
            return Err(TwtWireError::InvalidLength {
                expected: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
                actual: body.len(),
            });
        }
        if body[0] != TWT_ACTION_CATEGORY || body[1] != TWT_TEARDOWN_ACTION {
            return Err(TwtWireError::InvalidElement);
        }
        let flags = body[2];
        let negotiation_type = (flags & TEARDOWN_NEGOTIATION_TYPE) >> 5;
        if negotiation_type != 0 {
            return Err(TwtWireError::UnsupportedNegotiationType(negotiation_type));
        }
        if flags & TEARDOWN_RESERVED != 0 {
            return Err(TwtWireError::ReservedTeardownBits(flags));
        }
        let flow_value = flags & TEARDOWN_FLOW_ID;
        let all_flows = flags & TEARDOWN_ALL != 0;
        if all_flows && flow_value != 0 {
            return Err(TwtWireError::NonzeroFlowForTeardownAll(flow_value));
        }
        Ok(Self {
            flow_id: IndividualTwtFlowId::new(flow_value)
                .ok_or(TwtWireError::InvalidFlowId(flow_value))?,
            all_flows,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtAction {
    Setup(IndividualTwtSetup),
    Teardown(IndividualTwtTeardown),
}

/// True only for the two action codes owned by this codec.
pub fn is_individual_twt_action_candidate(body: &[u8]) -> bool {
    body.first() == Some(&TWT_ACTION_CATEGORY)
        && matches!(
            body.get(1),
            Some(&TWT_SETUP_ACTION) | Some(&TWT_TEARDOWN_ACTION)
        )
}

pub fn parse_individual_twt_action(body: &[u8]) -> Result<IndividualTwtAction, TwtWireError> {
    if body.len() < 2 || body[0] != TWT_ACTION_CATEGORY {
        return Err(TwtWireError::InvalidElement);
    }
    match body[1] {
        TWT_SETUP_ACTION => IndividualTwtSetup::parse(body).map(IndividualTwtAction::Setup),
        TWT_TEARDOWN_ACTION => {
            IndividualTwtTeardown::parse(body).map(IndividualTwtAction::Teardown)
        }
        action => Err(TwtWireError::UnsupportedAction(action)),
    }
}
