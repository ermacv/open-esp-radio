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
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

pub const TWT_ACTION_CATEGORY: u8 = 22;
pub const TWT_SETUP_ACTION: u8 = 6;
pub const TWT_TEARDOWN_ACTION: u8 = 7;
pub const TWT_ELEMENT_ID: u8 = 216;
pub const INDIVIDUAL_TWT_ELEMENT_BODY_LEN: u8 = 15;
pub const INDIVIDUAL_TWT_SETUP_BODY_LEN: usize = 20;
pub const INDIVIDUAL_TWT_TEARDOWN_BODY_LEN: usize = 3;
pub const INDIVIDUAL_TWT_FLOW_CAPACITY: usize = 8;

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
        TwtControlFields::from_bytes([0])
            .with_responder_pm(self.responder_power_save)
            .with_information_disabled(self.information_frames_disabled)
            .with_duration_unit(matches!(
                self.wake_duration_unit,
                TwtWakeDurationUnit::Tu1024
            ))
            .to_bytes()[0]
    }

    const fn parse(value: u8) -> Result<Self, TwtWireError> {
        let fields = TwtControlFields::from_bytes([value]);
        let negotiation_type = fields.negotiation_type();
        if negotiation_type != 0 {
            return Err(TwtWireError::UnsupportedNegotiationType(negotiation_type));
        }
        if fields.ndp_paging() || fields.reserved() != 0 {
            return Err(TwtWireError::UnsupportedControl(value));
        }
        Ok(Self {
            responder_power_save: fields.responder_pm(),
            information_frames_disabled: fields.information_disabled(),
            wake_duration_unit: if !fields.duration_unit() {
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
        Ok((self.nominal_minimum_wake_duration as u32)
            .saturating_mul(control.wake_duration_unit.micros()))
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

    fn encode_request_type(self) -> Result<u16, TwtWireError> {
        let fields = TwtRequestFields::from_bytes([0; 2])
            .with_requesting_sta(self.requesting_sta)
            .with_trigger(self.trigger)
            .with_implicit(self.implicit)
            .with_flow_type(matches!(self.flow_type, IndividualTwtFlowType::Unannounced))
            .with_protection(self.protection);
        let fields = fields
            .with_setup_command(self.setup_command as u8)
            .map_err(|_| TwtWireError::InvalidSetupCommand(self.setup_command as u8))?;
        let fields = fields
            .with_flow_id(self.flow_id.get())
            .map_err(|_| TwtWireError::InvalidFlowId(self.flow_id.get()))?;
        let fields = fields
            .with_wake_interval_exponent(self.wake_interval_exponent)
            .map_err(|_| TwtWireError::InvalidWakeIntervalExponent(self.wake_interval_exponent))?;
        Ok(u16::from_le_bytes(fields.to_bytes()))
    }

    fn parse(request_type: u16, bytes: &[u8; 12]) -> Result<Self, TwtWireError> {
        let [t0, t1, t2, t3, t4, t5, t6, t7, duration, m0, m1, channel] = *bytes;
        let fields = TwtRequestFields::from_bytes(request_type.to_le_bytes());
        let flow_value = fields.flow_id();
        let flow_id =
            IndividualTwtFlowId::new(flow_value).ok_or(TwtWireError::InvalidFlowId(flow_value))?;
        let setup_value = fields.setup_command();
        Ok(Self {
            requesting_sta: fields.requesting_sta(),
            setup_command: IndividualTwtSetupCommand::parse(setup_value)?,
            trigger: fields.trigger(),
            implicit: fields.implicit(),
            flow_type: if !fields.flow_type() {
                IndividualTwtFlowType::Announced
            } else {
                IndividualTwtFlowType::Unannounced
            },
            flow_id,
            wake_interval_exponent: fields.wake_interval_exponent(),
            protection: fields.protection(),
            target_wake_time_tsf: u64::from_le_bytes([t0, t1, t2, t3, t4, t5, t6, t7]),
            nominal_minimum_wake_duration: duration,
            wake_interval_mantissa: u16::from_le_bytes([m0, m1]),
            twt_channel: channel,
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
        let available = output.len();
        let Some(target) = output.first_chunk_mut::<INDIVIDUAL_TWT_SETUP_BODY_LEN>() else {
            return Err(TwtWireError::OutputTooSmall {
                required: INDIVIDUAL_TWT_SETUP_BODY_LEN,
                available,
            });
        };

        let mut body = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        body[0] = TWT_ACTION_CATEGORY;
        body[1] = TWT_SETUP_ACTION;
        body[2] = self.dialog_token;
        body[3] = TWT_ELEMENT_ID;
        body[4] = INDIVIDUAL_TWT_ELEMENT_BODY_LEN;
        body[5] = self.control.encode();
        crate::encoding::write_array::<6, _, _>(
            &mut body,
            &self.parameters.encode_request_type()?.to_le_bytes(),
        );
        crate::encoding::write_array::<8, _, _>(
            &mut body,
            &self.parameters.target_wake_time_tsf.to_le_bytes(),
        );
        body[16] = self.parameters.nominal_minimum_wake_duration;
        crate::encoding::write_array::<17, _, _>(
            &mut body,
            &self.parameters.wake_interval_mantissa.to_le_bytes(),
        );
        body[19] = self.parameters.twt_channel;
        *target = body;
        Ok(INDIVIDUAL_TWT_SETUP_BODY_LEN)
    }

    pub fn encode_body(self) -> Result<[u8; INDIVIDUAL_TWT_SETUP_BODY_LEN], TwtWireError> {
        let mut body = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        self.encode(&mut body)?;
        Ok(body)
    }

    pub fn parse(body: &[u8]) -> Result<Self, TwtWireError> {
        let &[
            category,
            action,
            dialog_token,
            element_id,
            element_length,
            control,
            request_type_low,
            request_type_high,
            parameters @ ..,
        ] = <&[u8; INDIVIDUAL_TWT_SETUP_BODY_LEN]>::try_from(body).map_err(|_| {
            TwtWireError::InvalidLength {
                expected: INDIVIDUAL_TWT_SETUP_BODY_LEN,
                actual: body.len(),
            }
        })?;
        if category != TWT_ACTION_CATEGORY
            || action != TWT_SETUP_ACTION
            || element_id != TWT_ELEMENT_ID
            || element_length != INDIVIDUAL_TWT_ELEMENT_BODY_LEN
        {
            return Err(TwtWireError::InvalidElement);
        }
        let control = IndividualTwtControl::parse(control)?;
        let request_type = u16::from_le_bytes([request_type_low, request_type_high]);
        let parameters = IndividualTwtParameterSet::parse(request_type, &parameters)?;
        Self {
            dialog_token,
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
        let available = output.len();
        let Some(target) = output.first_chunk_mut::<INDIVIDUAL_TWT_TEARDOWN_BODY_LEN>() else {
            return Err(TwtWireError::OutputTooSmall {
                required: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
                available,
            });
        };
        let body = [
            TWT_ACTION_CATEGORY,
            TWT_TEARDOWN_ACTION,
            TwtTeardownFields::from_bytes([self.flow_id.get()])
                .with_all(self.all_flows)
                .to_bytes()[0],
        ];
        *target = body;
        Ok(INDIVIDUAL_TWT_TEARDOWN_BODY_LEN)
    }

    pub fn encode_body(self) -> Result<[u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN], TwtWireError> {
        let mut body = [0_u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN];
        self.encode(&mut body)?;
        Ok(body)
    }

    pub fn parse(body: &[u8]) -> Result<Self, TwtWireError> {
        let &[category, action, flags] = <&[u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN]>::try_from(body)
            .map_err(|_| TwtWireError::InvalidLength {
                expected: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
                actual: body.len(),
            })?;
        if category != TWT_ACTION_CATEGORY || action != TWT_TEARDOWN_ACTION {
            return Err(TwtWireError::InvalidElement);
        }
        let negotiation_type = TwtTeardownFields::from_bytes([flags]).negotiation_type();
        if negotiation_type != 0 {
            return Err(TwtWireError::UnsupportedNegotiationType(negotiation_type));
        }
        if TwtTeardownFields::from_bytes([flags]).reserved() != 0 {
            return Err(TwtWireError::ReservedTeardownBits(flags));
        }
        let flow_value = TwtTeardownFields::from_bytes([flags]).flow_id();
        let all_flows = TwtTeardownFields::from_bytes([flags]).all();
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
    let Some(&[category, action]) = body.first_chunk::<2>() else {
        return Err(TwtWireError::InvalidElement);
    };
    if category != TWT_ACTION_CATEGORY {
        return Err(TwtWireError::InvalidElement);
    }
    match action {
        TWT_SETUP_ACTION => IndividualTwtSetup::parse(body).map(IndividualTwtAction::Setup),
        TWT_TEARDOWN_ACTION => {
            IndividualTwtTeardown::parse(body).map(IndividualTwtAction::Teardown)
        }
        action => Err(TwtWireError::UnsupportedAction(action)),
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.199 (TWT element).
    pub struct TwtControlFields[1] (little) {
        pub ndp_paging, with_ndp_paging: bool = 0..1;
        pub responder_pm, with_responder_pm: bool = 1..2;
        pub negotiation_type, with_negotiation_type: u8 = 2..4;
        pub information_disabled, with_information_disabled: bool = 4..5;
        pub duration_unit, with_duration_unit: bool = 5..6;
        pub reserved, with_reserved: u8 = 6..8;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.199 (TWT Request Type field).
    pub struct TwtRequestFields[2] (little) {
        pub requesting_sta, with_requesting_sta: bool = 0..1;
        pub setup_command, with_setup_command: u8 = 1..4;
        pub trigger, with_trigger: bool = 4..5;
        pub implicit, with_implicit: bool = 5..6;
        pub flow_type, with_flow_type: bool = 6..7;
        pub flow_id, with_flow_id: u8 = 7..10;
        pub wake_interval_exponent, with_wake_interval_exponent: u8 = 10..15;
        pub protection, with_protection: bool = 15..16;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, Figure 9-965 (TWT Teardown field).
    pub struct TwtTeardownFields[1] (little) {
        pub flow_id, with_flow_id: u8 = 0..3;
        pub reserved, with_reserved: u8 = 3..5;
        pub negotiation_type, with_negotiation_type: u8 = 5..7;
        pub all, with_all: bool = 7..8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_setup_still_requires_a_representable_exponent_before_writing() {
        let mut setup = IndividualTwtSetup {
            dialog_token: 9,
            control: IndividualTwtControl::REQUEST,
            parameters: IndividualTwtParameterSet {
                requesting_sta: false,
                setup_command: IndividualTwtSetupCommand::Reject,
                trigger: false,
                implicit: false,
                flow_type: IndividualTwtFlowType::Announced,
                flow_id: IndividualTwtFlowId::ALL[0],
                wake_interval_exponent: 31,
                protection: false,
                target_wake_time_tsf: 0,
                nominal_minimum_wake_duration: 0,
                wake_interval_mantissa: 0,
                twt_channel: 0,
            },
        };
        assert_eq!(
            setup
                .encode_body()
                .and_then(|body| IndividualTwtSetup::parse(&body)),
            Ok(setup)
        );
        for exponent in [32, 255] {
            setup.parameters.wake_interval_exponent = exponent;
            let mut output = [0xa5; INDIVIDUAL_TWT_SETUP_BODY_LEN];
            assert_eq!(
                setup.encode(&mut output),
                Err(TwtWireError::InvalidWakeIntervalExponent(exponent))
            );
            assert_eq!(output, [0xa5; INDIVIDUAL_TWT_SETUP_BODY_LEN]);
        }
    }

    #[test]
    fn every_prefix_of_an_action_body_is_handled() {
        let mut setup = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        setup[0] = TWT_ACTION_CATEGORY;
        setup[1] = TWT_SETUP_ACTION;
        setup[3] = TWT_ELEMENT_ID;
        setup[4] = INDIVIDUAL_TWT_ELEMENT_BODY_LEN;
        let teardown = [TWT_ACTION_CATEGORY, TWT_TEARDOWN_ACTION, 0];
        for len in 0..=setup.len() {
            let setup = &setup[..len];
            let _ = parse_individual_twt_action(setup);
            let _ = IndividualTwtSetup::parse(setup);
            let _ = IndividualTwtTeardown::parse(&teardown[..len.min(teardown.len())]);
        }
    }

    #[test]
    fn an_output_too_small_for_a_body_is_reported() {
        let teardown = IndividualTwtTeardown::all();
        let mut output = [0_u8; INDIVIDUAL_TWT_TEARDOWN_BODY_LEN - 1];
        assert_eq!(
            teardown.encode(&mut output),
            Err(TwtWireError::OutputTooSmall {
                required: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN,
                available: INDIVIDUAL_TWT_TEARDOWN_BODY_LEN - 1,
            })
        );
    }
}
