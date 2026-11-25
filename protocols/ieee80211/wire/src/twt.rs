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

use crate::action::{ActionCategory, S1gAction};
use crate::element::ElementId;
use crate::management::DialogToken;

pub const INDIVIDUAL_TWT_ELEMENT_BODY_LEN: u8 = 15;
pub const INDIVIDUAL_TWT_SETUP_BODY_LEN: usize = 20;
pub const INDIVIDUAL_TWT_TEARDOWN_BODY_LEN: usize = 3;
pub const INDIVIDUAL_TWT_FLOW_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TwtWireError {
    OutputTooSmall { required: usize, available: usize },
    InvalidLength { expected: usize, actual: usize },
    InvalidElement,
    UnsupportedAction(S1gAction),
    UnsupportedControl(u8),
    UnsupportedNegotiationType(u8),
    InvalidFlowId(u8),
    UnsupportedSetupCommand(IndividualTwtSetupCommand),
    RequestCommandFromResponder,
    ResponseCommandFromRequester,
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
    /// The flow a three-bit Flow Identifier field names: each of its eight
    /// values names one.
    pub const fn from_field(field: oer_portable_wire::U8<3>) -> Self {
        Self(match field.get() {
            0 => FlowNumber::Flow0,
            1 => FlowNumber::Flow1,
            2 => FlowNumber::Flow2,
            3 => FlowNumber::Flow3,
            4 => FlowNumber::Flow4,
            5 => FlowNumber::Flow5,
            6 => FlowNumber::Flow6,
            _ => FlowNumber::Flow7,
        })
    }
    /// The flow's three-bit Flow Identifier field.
    pub const fn field(self) -> oer_portable_wire::U8<3> {
        match self.0 {
            FlowNumber::Flow0 => oer_portable_wire::U8::constant::<0>(),
            FlowNumber::Flow1 => oer_portable_wire::U8::constant::<1>(),
            FlowNumber::Flow2 => oer_portable_wire::U8::constant::<2>(),
            FlowNumber::Flow3 => oer_portable_wire::U8::constant::<3>(),
            FlowNumber::Flow4 => oer_portable_wire::U8::constant::<4>(),
            FlowNumber::Flow5 => oer_portable_wire::U8::constant::<5>(),
            FlowNumber::Flow6 => oer_portable_wire::U8::constant::<6>(),
            FlowNumber::Flow7 => oer_portable_wire::U8::constant::<7>(),
        }
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
    /// The command a three-bit TWT Setup Command field names: each of its
    /// eight values names one.
    pub const fn from_field(field: oer_portable_wire::U8<3>) -> Self {
        match field.get() {
            0 => Self::Request,
            1 => Self::Suggest,
            2 => Self::Demand,
            3 => Self::Grouping,
            4 => Self::Accept,
            5 => Self::Alternate,
            6 => Self::Dictate,
            _ => Self::Reject,
        }
    }

    /// The command's three-bit TWT Setup Command field.
    pub const fn field(self) -> oer_portable_wire::U8<3> {
        match self {
            Self::Request => oer_portable_wire::U8::constant::<0>(),
            Self::Suggest => oer_portable_wire::U8::constant::<1>(),
            Self::Demand => oer_portable_wire::U8::constant::<2>(),
            Self::Grouping => oer_portable_wire::U8::constant::<3>(),
            Self::Accept => oer_portable_wire::U8::constant::<4>(),
            Self::Alternate => oer_portable_wire::U8::constant::<5>(),
            Self::Dictate => oer_portable_wire::U8::constant::<6>(),
            Self::Reject => oer_portable_wire::U8::constant::<7>(),
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

/// The Trigger subfield of a TWT Request Type field: the agreement's
/// service periods carry Trigger frames, or none (IEEE Std 802.11-2020,
/// 9.4.2.199).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtTrigger {
    TriggerEnabled,
    NonTriggerEnabled,
}

/// The Implicit subfield of a TWT Request Type field: each next wake follows
/// by the wake interval, or each is announced explicitly (IEEE Std
/// 802.11-2020, 9.4.2.199).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtImplicit {
    Implicit,
    Explicit,
}

/// The TWT Protection subfield of a TWT Request Type field (IEEE Std
/// 802.11-2020, 9.4.2.199).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualTwtProtection {
    Protected,
    Unprotected,
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

impl TwtControlFields {
    /// A requester's control: no responder power save, information frames
    /// enabled, a wake duration in units of 256 us.
    pub const REQUEST: Self = Self::from_bytes([0]);

    /// The unit of the Nominal Minimum TWT Wake Duration.
    pub const fn wake_duration_unit(self) -> TwtWakeDurationUnit {
        if self.duration_unit() {
            TwtWakeDurationUnit::Tu1024
        } else {
            TwtWakeDurationUnit::Micros256
        }
    }

    /// The control field of an individual TWT of negotiation type zero,
    /// without NDP paging and with its reserved bits clear: the supported
    /// fixed portion.
    const fn supported(value: u8) -> Result<Self, TwtWireError> {
        let fields = Self::from_bytes([value]);
        let negotiation_type = fields.negotiation_type();
        if negotiation_type != 0 {
            return Err(TwtWireError::UnsupportedNegotiationType(negotiation_type));
        }
        if fields.ndp_paging() || fields.reserved() != 0 {
            return Err(TwtWireError::UnsupportedControl(value));
        }
        Ok(fields)
    }
}

/// One fixed individual TWT parameter set: its Request Type field and the
/// fields after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtParameterSet {
    pub request: TwtRequestFields,
    pub target_wake_time_tsf: u64,
    pub nominal_minimum_wake_duration: u8,
    pub wake_interval_mantissa: u16,
    pub twt_channel: u8,
}

impl IndividualTwtParameterSet {
    pub const fn requesting_sta(self) -> bool {
        self.request.requesting_sta()
    }

    pub const fn setup_command(self) -> IndividualTwtSetupCommand {
        IndividualTwtSetupCommand::from_field(self.request.setup_command())
    }

    pub const fn trigger(self) -> IndividualTwtTrigger {
        if self.request.trigger() {
            IndividualTwtTrigger::TriggerEnabled
        } else {
            IndividualTwtTrigger::NonTriggerEnabled
        }
    }

    pub const fn implicit(self) -> IndividualTwtImplicit {
        if self.request.implicit() {
            IndividualTwtImplicit::Implicit
        } else {
            IndividualTwtImplicit::Explicit
        }
    }

    pub const fn flow_type(self) -> IndividualTwtFlowType {
        if self.request.flow_type() {
            IndividualTwtFlowType::Unannounced
        } else {
            IndividualTwtFlowType::Announced
        }
    }

    pub const fn flow_id(self) -> IndividualTwtFlowId {
        IndividualTwtFlowId::from_field(self.request.flow_id())
    }

    pub const fn protection(self) -> IndividualTwtProtection {
        if self.request.protection() {
            IndividualTwtProtection::Protected
        } else {
            IndividualTwtProtection::Unprotected
        }
    }

    pub const fn wake_interval_micros(self) -> Result<u64, TwtWireError> {
        if self.wake_interval_mantissa == 0 {
            return Err(TwtWireError::ZeroWakeIntervalMantissa);
        }
        // The exponent's five bits shift a sixteen-bit mantissa within 64.
        Ok((self.wake_interval_mantissa as u64) << self.request.wake_interval_exponent().get())
    }

    pub const fn wake_duration_micros(
        self,
        control: TwtControlFields,
    ) -> Result<u32, TwtWireError> {
        if self.nominal_minimum_wake_duration == 0 {
            return Err(TwtWireError::ZeroWakeDuration);
        }
        Ok((self.nominal_minimum_wake_duration as u32)
            .saturating_mul(control.wake_duration_unit().micros()))
    }

    pub fn validate(self, control: TwtControlFields) -> Result<Self, TwtWireError> {
        let command = self.setup_command();
        if matches!(command, IndividualTwtSetupCommand::Grouping) {
            return Err(TwtWireError::UnsupportedSetupCommand(
                IndividualTwtSetupCommand::Grouping,
            ));
        }
        if self.requesting_sta() && !command.is_requester_command() {
            return Err(TwtWireError::ResponseCommandFromRequester);
        }
        if !self.requesting_sta() && !command.is_responder_command() {
            return Err(TwtWireError::RequestCommandFromResponder);
        }
        if self.twt_channel != 0 {
            return Err(TwtWireError::UnsupportedTwtChannel(self.twt_channel));
        }
        if command != IndividualTwtSetupCommand::Reject {
            let interval = self.wake_interval_micros()?;
            let duration = self.wake_duration_micros(control)?;
            if duration as u64 > interval {
                return Err(TwtWireError::WakeDurationExceedsInterval);
            }
        }
        Ok(self)
    }

    fn parse(request: TwtRequestFields, bytes: [u8; 12]) -> Self {
        let fields = IndividualTwtFields::from_bytes(bytes);
        Self {
            request,
            target_wake_time_tsf: fields.target_wake_time(),
            nominal_minimum_wake_duration: fields.nominal_minimum_wake_duration(),
            wake_interval_mantissa: fields.wake_interval_mantissa(),
            twt_channel: fields.twt_channel(),
        }
    }

    /// The fields after the Request Type, as they appear on air.
    const fn fields(self) -> IndividualTwtFields {
        IndividualTwtFields::from_bytes([0; 12])
            .with_target_wake_time(self.target_wake_time_tsf)
            .with_nominal_minimum_wake_duration(self.nominal_minimum_wake_duration)
            .with_wake_interval_mantissa(self.wake_interval_mantissa)
            .with_twt_channel(self.twt_channel)
    }
}

/// Complete body of one individual TWT Setup Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndividualTwtSetup {
    pub dialog_token: DialogToken,
    pub control: TwtControlFields,
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

        let [control] = self.control.to_bytes();
        let [r0, r1] = self.parameters.request.to_bytes();
        let [p0, p1, p2, p3, p4, p5, p6, p7, p8, p9, p10, p11] =
            self.parameters.fields().to_bytes();
        // SOURCE: IEEE Std 802.11-2020, 9.6.24.2 (TWT Setup frame format):
        // Category, S1G Action, Dialog Token, then the TWT element (9.4.2.199)
        // with Control, Request Type, Target Wake Time, Nominal Minimum TWT
        // Wake Duration, TWT Wake Interval Mantissa and TWT Channel.
        let body = [
            ActionCategory::S1G.value(),
            S1gAction::TWT_SETUP.value(),
            self.dialog_token.value(),
            ElementId::TWT.value(),
            INDIVIDUAL_TWT_ELEMENT_BODY_LEN,
            control,
            r0,
            r1,
            p0,
            p1,
            p2,
            p3,
            p4,
            p5,
            p6,
            p7,
            p8,
            p9,
            p10,
            p11,
        ];
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
        if ActionCategory::new(category) != ActionCategory::S1G
            || S1gAction::new(action) != S1gAction::TWT_SETUP
            || ElementId::new(element_id) != ElementId::TWT
            || element_length != INDIVIDUAL_TWT_ELEMENT_BODY_LEN
        {
            return Err(TwtWireError::InvalidElement);
        }
        let control = TwtControlFields::supported(control)?;
        let parameters = IndividualTwtParameterSet::parse(
            TwtRequestFields::from_bytes([request_type_low, request_type_high]),
            parameters,
        );
        Self {
            dialog_token: DialogToken::new(dialog_token),
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
            ActionCategory::S1G.value(),
            S1gAction::TWT_TEARDOWN.value(),
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
        if ActionCategory::new(category) != ActionCategory::S1G
            || S1gAction::new(action) != S1gAction::TWT_TEARDOWN
        {
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
    let Some(&[category, action]) = body.first_chunk::<2>() else {
        return false;
    };
    ActionCategory::new(category) == ActionCategory::S1G
        && matches!(
            S1gAction::new(action),
            S1gAction::TWT_SETUP | S1gAction::TWT_TEARDOWN
        )
}

pub fn parse_individual_twt_action(body: &[u8]) -> Result<IndividualTwtAction, TwtWireError> {
    let Some(&[category, action]) = body.first_chunk::<2>() else {
        return Err(TwtWireError::InvalidElement);
    };
    if ActionCategory::new(category) != ActionCategory::S1G {
        return Err(TwtWireError::InvalidElement);
    }
    match S1gAction::new(action) {
        S1gAction::TWT_SETUP => IndividualTwtSetup::parse(body).map(IndividualTwtAction::Setup),
        S1gAction::TWT_TEARDOWN => {
            IndividualTwtTeardown::parse(body).map(IndividualTwtAction::Teardown)
        }
        action => Err(TwtWireError::UnsupportedAction(action)),
    }
}

oer_portable_wire::bitfield! {
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

oer_portable_wire::bitfield! {
    /// An individual TWT parameter set's fields after its Request Type.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.199 (TWT element, Figure 9-686):
    /// Target Wake Time, Nominal Minimum TWT Wake Duration, TWT Wake
    /// Interval Mantissa and TWT Channel.
    pub struct IndividualTwtFields[12] (little) {
        pub target_wake_time, with_target_wake_time: {u64} = 0..64;
        pub nominal_minimum_wake_duration, with_nominal_minimum_wake_duration: {u8} = 64..72;
        pub wake_interval_mantissa, with_wake_interval_mantissa: {u16} = 72..88;
        pub twt_channel, with_twt_channel: {u8} = 88..96;
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.199 (TWT Request Type field).
    pub struct TwtRequestFields[2] (little) {
        pub requesting_sta, with_requesting_sta: bool = 0..1;
        pub setup_command, with_setup_command: (u8) = 1..4;
        pub trigger, with_trigger: bool = 4..5;
        pub implicit, with_implicit: bool = 5..6;
        pub flow_type, with_flow_type: bool = 6..7;
        pub flow_id, with_flow_id: (u8) = 7..10;
        pub wake_interval_exponent, with_wake_interval_exponent: (u8) = 10..15;
        pub protection, with_protection: bool = 15..16;
    }
}

oer_portable_wire::bitfield! {
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

    /// Each flow and setup command is its three-bit field, both ways.
    #[test]
    fn flows_and_commands_are_their_fields() {
        for (index, flow) in IndividualTwtFlowId::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(flow.field().get()), index);
            assert_eq!(IndividualTwtFlowId::from_field(flow.field()), flow);
        }
        for value in 0..8 {
            let field = oer_portable_wire::U8::<3>::new(value).unwrap();
            assert_eq!(IndividualTwtSetupCommand::from_field(field).field(), field);
        }
    }

    #[test]
    fn a_rejected_setup_with_the_largest_exponent_round_trips() {
        let setup = IndividualTwtSetup {
            dialog_token: DialogToken::new(9),
            control: TwtControlFields::REQUEST,
            parameters: IndividualTwtParameterSet {
                request: TwtRequestFields::from_bytes([0; 2])
                    .with_setup_command(IndividualTwtSetupCommand::Reject.field())
                    .with_flow_id(IndividualTwtFlowId::ALL[0].field())
                    .with_wake_interval_exponent(oer_portable_wire::U8::constant::<31>()),
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
    }

    #[test]
    fn a_setup_body_has_the_bytes_of_every_field() {
        let setup = IndividualTwtSetup {
            dialog_token: DialogToken::new(9),
            control: TwtControlFields::REQUEST
                .with_responder_pm(true)
                .with_information_disabled(true)
                .with_duration_unit(true),
            parameters: IndividualTwtParameterSet {
                request: TwtRequestFields::from_bytes([0; 2])
                    .with_requesting_sta(true)
                    .with_setup_command(IndividualTwtSetupCommand::Demand.field())
                    .with_trigger(true)
                    .with_implicit(true)
                    .with_flow_type(true)
                    .with_flow_id(IndividualTwtFlowId::ALL[5].field())
                    .with_wake_interval_exponent(oer_portable_wire::U8::constant::<10>())
                    .with_protection(true),
                target_wake_time_tsf: 0x0123_4567_89ab_cdef,
                nominal_minimum_wake_duration: 0x40,
                wake_interval_mantissa: 0x1234,
                twt_channel: 0,
            },
        };
        let body = [
            0x16, 0x06, 0x09, 0xd8, 0x0f, 0x32, 0xf5, 0xaa, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45,
            0x23, 0x01, 0x40, 0x34, 0x12, 0x00,
        ];
        assert_eq!(setup.encode_body(), Ok(body));
        assert_eq!(IndividualTwtSetup::parse(&body), Ok(setup));
    }

    #[test]
    fn a_request_type_names_each_command_flow_and_flow_type() {
        for (index, &flow) in IndividualTwtFlowId::ALL.iter().enumerate() {
            let request = TwtRequestFields::from_bytes([0; 2]).with_flow_id(flow.field());
            let parameters = IndividualTwtParameterSet {
                request,
                target_wake_time_tsf: 0,
                nominal_minimum_wake_duration: 0,
                wake_interval_mantissa: 0,
                twt_channel: 0,
            };
            assert_eq!(parameters.flow_id(), flow);
            assert_eq!(usize::from(flow.get()), index);
        }
        for command in [
            IndividualTwtSetupCommand::Request,
            IndividualTwtSetupCommand::Suggest,
            IndividualTwtSetupCommand::Demand,
            IndividualTwtSetupCommand::Grouping,
            IndividualTwtSetupCommand::Accept,
            IndividualTwtSetupCommand::Alternate,
            IndividualTwtSetupCommand::Dictate,
            IndividualTwtSetupCommand::Reject,
        ] {
            assert_eq!(
                IndividualTwtSetupCommand::from_field(command.field()),
                command
            );
        }
        let parameters = |request| IndividualTwtParameterSet {
            request,
            target_wake_time_tsf: 0,
            nominal_minimum_wake_duration: 0,
            wake_interval_mantissa: 0,
            twt_channel: 0,
        };
        let announced = TwtRequestFields::from_bytes([0; 2]);
        assert_eq!(
            parameters(announced).flow_type(),
            IndividualTwtFlowType::Announced
        );
        assert_eq!(
            parameters(announced.with_flow_type(true)).flow_type(),
            IndividualTwtFlowType::Unannounced
        );
        assert_eq!(
            TwtControlFields::REQUEST.wake_duration_unit(),
            TwtWakeDurationUnit::Micros256
        );
        assert_eq!(
            TwtControlFields::REQUEST
                .with_duration_unit(true)
                .wake_duration_unit(),
            TwtWakeDurationUnit::Tu1024
        );
    }

    /// The category and the action each refuse a body alone, for either
    /// frame and for the parser of both.
    #[test]
    fn a_body_of_another_category_or_action_is_refused() {
        let setup = |category, action| {
            let mut body = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
            body[..5].copy_from_slice(&[
                category,
                action,
                1,
                ElementId::TWT.value(),
                INDIVIDUAL_TWT_ELEMENT_BODY_LEN,
            ]);
            body
        };
        for (category, action) in [(21, 6), (22, 7)] {
            assert_eq!(
                IndividualTwtSetup::parse(&setup(category, action)),
                Err(TwtWireError::InvalidElement)
            );
        }
        for (category, action) in [(21, 7), (22, 6)] {
            assert_eq!(
                IndividualTwtTeardown::parse(&[category, action, 0]),
                Err(TwtWireError::InvalidElement)
            );
        }
        assert!(IndividualTwtTeardown::parse(&[22, 7, 0]).is_ok());
        assert!(parse_individual_twt_action(&[22, 7, 0]).is_ok());
        assert_eq!(
            parse_individual_twt_action(&[21, 7, 0]),
            Err(TwtWireError::InvalidElement)
        );
        assert_eq!(
            parse_individual_twt_action(&[22, 5, 0]),
            Err(TwtWireError::UnsupportedAction(S1gAction::new(5)))
        );
        for (body, candidate) in [
            (&[22, 6][..], true),
            (&[22, 7], true),
            (&[22, 5], false),
            (&[21, 6], false),
            (&[22], false),
            (&[], false),
        ] {
            assert_eq!(is_individual_twt_action_candidate(body), candidate);
        }
    }

    #[test]
    fn every_prefix_of_an_action_body_is_handled() {
        let mut setup = [0_u8; INDIVIDUAL_TWT_SETUP_BODY_LEN];
        setup[0] = ActionCategory::S1G.value();
        setup[1] = S1gAction::TWT_SETUP.value();
        setup[3] = ElementId::TWT.value();
        setup[4] = INDIVIDUAL_TWT_ELEMENT_BODY_LEN;
        let teardown = [
            ActionCategory::S1G.value(),
            S1gAction::TWT_TEARDOWN.value(),
            0,
        ];
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
