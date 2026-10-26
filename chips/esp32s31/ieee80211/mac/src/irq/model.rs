//! Semantic models of the MAC and WDEVPWR interrupt banks.
//!
//! Each model keeps what the hardware keeps: latched events, the enable
//! state, and the masked STATUS image that the handler samples (both banks'
//! STATUS registers are the latched events under their enable mask). A
//! latched event survives masking and becomes visible again when its source
//! is unmasked; only an acknowledgement removes it. `line_asserted` is the
//! level the bank drives towards its CPU interrupt line. Every bank operation
//! is recorded in a bounded log.
//!
//! The MAC model groups its sources as the epoch does: the RX delivery group
//! (RX success and the two RX-associated auxiliary sources) and every other
//! source of the runtime mask (TX completion, TX timeout, BSS-color collision and the
//! still-unidentified sources). The WDEVPWR model enables only the station
//! TBTT, as epoch activation does. Masked MAC closure acknowledges only its
//! captured RAW events; WDEVPWR closure clears every latched event.

use oer_esp32s31_hal::types::{
    MacInterruptEnableState, MacInterruptEvents, MacInterruptMask, MacInterruptObservation,
    MacPowerInterruptObservation,
};

use super::{BanksMasked, MacInterrupt, MacInterruptRawSample, MacPowerInterrupt, sealed};

/// Operations a log keeps; later ones are counted in
/// [`OperationLog::overflowed`].
pub const OPERATION_LOG_CAPACITY: usize = 32;

/// One operation on the MAC interrupt bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptOperation {
    Status,
    Raw,
    MaskRxDelivery,
    UnmaskRxDelivery,
    MaskAll,
    Acknowledge(MacInterruptObservation),
    AcknowledgeRaw,
    Publish(MacInterruptMask),
}

/// One operation on the WDEVPWR interrupt bank.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerInterruptOperation {
    Status,
    MaskAll,
    Acknowledge(MacPowerInterruptObservation),
    ClearAll,
    EnableStationTbtt,
}

/// The bounded, allocation-free log of one bank's operations.
#[derive(Clone, Debug)]
pub struct OperationLog<T: Copy> {
    entries: [Option<T>; OPERATION_LOG_CAPACITY],
    len: usize,
    overflowed: u32,
}

impl<T: Copy> OperationLog<T> {
    const fn new() -> Self {
        Self {
            entries: [const { None }; OPERATION_LOG_CAPACITY],
            len: 0,
            overflowed: 0,
        }
    }

    fn push(&mut self, operation: T) {
        if let Some(entry) = self.entries.get_mut(self.len) {
            *entry = Some(operation);
            self.len += 1;
        } else {
            self.overflowed = self.overflowed.saturating_add(1);
        }
    }

    /// The kept operations, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = T> + '_ {
        self.entries.iter().take(self.len).flatten().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Operations recorded after the log was full.
    pub const fn overflowed(&self) -> u32 {
        self.overflowed
    }

    /// Whether the kept operations are exactly `expected` and none overflowed.
    pub fn matches(&self, expected: &[T]) -> bool
    where
        T: PartialEq,
    {
        self.overflowed == 0 && self.iter().eq(expected.iter().copied())
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

const fn events_without(
    events: MacInterruptEvents,
    removed: MacInterruptEvents,
) -> MacInterruptEvents {
    MacInterruptEvents::from_causes(
        events.tx_complete() && !removed.tx_complete(),
        events.bss_color_collision() && !removed.bss_color_collision(),
        events.rx_success() && !removed.rx_success(),
        events.tx_timeout() && !removed.tx_timeout(),
    )
}

/// An owned RAW snapshot of the MAC model, including masked pending events.
pub struct MacRawSnapshot {
    events: MacInterruptObservation,
    fatal: Option<u32>,
}

impl sealed::Sealed for MacRawSnapshot {}
impl MacInterruptRawSample for MacRawSnapshot {
    fn fatal_assertion_raw(&self) -> Option<u32> {
        self.fatal
    }
}

/// The MAC interrupt bank of one active epoch.
#[derive(Clone, Debug)]
pub struct MacInterruptModel {
    latched: MacInterruptObservation,
    fatal_assertion_raw: Option<u32>,
    rx_delivery_enabled: bool,
    other_sources_enabled: bool,
    operations: OperationLog<MacInterruptOperation>,
}

impl MacInterruptModel {
    /// The bank as station epoch activation leaves it: RX and TX enabled,
    /// the HE BSS-color source masked, nothing latched.
    pub const fn new() -> Self {
        Self {
            latched: MacInterruptObservation::from_semantic_events(
                MacInterruptEvents::empty(),
                false,
                false,
            ),
            fatal_assertion_raw: None,
            rx_delivery_enabled: true,
            other_sources_enabled: true,
            operations: OperationLog::new(),
        }
    }

    /// The bank with every source masked and nothing latched.
    pub const fn masked() -> Self {
        let mut model = Self::new();
        model.rx_delivery_enabled = false;
        model.other_sources_enabled = false;
        model
    }

    /// The hardware latches `events`, whatever the enable state.
    pub fn latch(&mut self, events: MacInterruptObservation) {
        self.latched = MacInterruptObservation::from_semantic_events(
            self.latched.work_events().union(events.work_events()),
            self.latched.has_auxiliary_event() || events.has_auxiliary_event(),
            self.latched.has_unhandled_event() || events.has_unhandled_event(),
        );
    }

    /// Inject a named fatal RAW cause without depending on register geometry.
    /// The sample stays observable under every delivery mask.
    pub fn latch_fatal_assertion(&mut self, raw: u32) {
        if self.fatal_assertion_raw.is_none() {
            self.fatal_assertion_raw = Some(raw);
        }
    }

    pub const fn latched_fatal_assertion_raw(&self) -> Option<u32> {
        self.fatal_assertion_raw
    }

    /// Every latched event, masked or not.
    pub const fn latched(&self) -> MacInterruptObservation {
        self.latched
    }

    /// The STATUS image: the latched events whose source is enabled.
    pub const fn visible(&self) -> MacInterruptObservation {
        let latched = self.latched.work_events();
        let rx = self.rx_delivery_enabled;
        let other = self.other_sources_enabled;
        MacInterruptObservation::from_semantic_events(
            MacInterruptEvents::from_causes(
                other && latched.tx_complete(),
                false, // The station mask excludes the HE color source.
                rx && latched.rx_success(),
                other && latched.tx_timeout(),
            ),
            rx && self.latched.has_auxiliary_event(),
            other && self.latched.has_unhandled_event(),
        )
    }

    pub const fn rx_delivery_enabled(&self) -> bool {
        self.rx_delivery_enabled
    }

    /// Whether any source outside the RX delivery group is enabled.
    pub const fn other_sources_enabled(&self) -> bool {
        self.other_sources_enabled
    }

    /// The level this bank drives towards its CPU interrupt line.
    pub const fn line_asserted(&self) -> bool {
        !self.visible().is_empty()
            || (self.other_sources_enabled && self.fatal_assertion_raw.is_some())
    }

    pub const fn operations(&self) -> &OperationLog<MacInterruptOperation> {
        &self.operations
    }

    pub fn clear_operations(&mut self) {
        self.operations.clear();
    }
}

impl Default for MacInterruptModel {
    fn default() -> Self {
        Self::new()
    }
}

impl sealed::Sealed for MacInterruptModel {}
impl MacInterrupt for MacInterruptModel {
    type Snapshot = MacInterruptObservation;
    type RawSnapshot = MacRawSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.operations.push(MacInterruptOperation::Status);
        self.visible()
    }

    fn raw(&mut self) -> Self::RawSnapshot {
        self.operations.push(MacInterruptOperation::Raw);
        MacRawSnapshot {
            events: self.latched,
            fatal: self.fatal_assertion_raw,
        }
    }

    fn mask_rx_delivery(&mut self) {
        self.operations.push(MacInterruptOperation::MaskRxDelivery);
        self.rx_delivery_enabled = false;
    }

    fn unmask_rx_delivery(&mut self) {
        self.operations
            .push(MacInterruptOperation::UnmaskRxDelivery);
        self.rx_delivery_enabled = true;
    }

    fn mask_all(&mut self) {
        self.operations.push(MacInterruptOperation::MaskAll);
        self.rx_delivery_enabled = false;
        self.other_sources_enabled = false;
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.operations
            .push(MacInterruptOperation::Acknowledge(snapshot));
        self.latched = MacInterruptObservation::from_semantic_events(
            events_without(self.latched.work_events(), snapshot.work_events()),
            self.latched.has_auxiliary_event() && !snapshot.has_auxiliary_event(),
            self.latched.has_unhandled_event() && !snapshot.has_unhandled_event(),
        );
    }

    fn acknowledge_raw(&mut self, snapshot: Self::RawSnapshot, _masked: &BanksMasked) {
        self.operations.push(MacInterruptOperation::AcknowledgeRaw);
        self.latched = MacInterruptObservation::from_semantic_events(
            events_without(self.latched.work_events(), snapshot.events.work_events()),
            self.latched.has_auxiliary_event() && !snapshot.events.has_auxiliary_event(),
            self.latched.has_unhandled_event() && !snapshot.events.has_unhandled_event(),
        );
        if snapshot.fatal.is_some() {
            self.fatal_assertion_raw = None;
        }
    }

    fn publish(&mut self, mask: MacInterruptMask) {
        self.operations.push(MacInterruptOperation::Publish(mask));
        let station = mask != MacInterruptMask::NONE;
        self.rx_delivery_enabled = station;
        self.other_sources_enabled = station;
    }

    fn enable_state(&self) -> MacInterruptEnableState {
        match (self.rx_delivery_enabled, self.other_sources_enabled) {
            (false, false) => MacInterruptEnableState::Disabled,
            (true, true) => MacInterruptEnableState::Station,
            _ => MacInterruptEnableState::Unknown,
        }
    }
}

/// The WDEVPWR interrupt bank of one active epoch.
#[derive(Clone, Debug)]
pub struct PowerInterruptModel {
    latched: MacPowerInterruptObservation,
    sta_tbtt_enabled: bool,
    operations: OperationLog<PowerInterruptOperation>,
}

impl PowerInterruptModel {
    /// The bank as epoch activation leaves it: only the station TBTT
    /// enabled, nothing latched.
    pub const fn new() -> Self {
        Self {
            latched: MacPowerInterruptObservation::from_semantic_events(
                false, false, false, false, false, false,
            ),
            sta_tbtt_enabled: true,
            operations: OperationLog::new(),
        }
    }

    /// The bank with every cause masked and nothing latched.
    pub const fn masked() -> Self {
        let mut model = Self::new();
        model.sta_tbtt_enabled = false;
        model
    }

    /// The hardware latches `causes`, whatever the enable state.
    pub fn latch(&mut self, causes: MacPowerInterruptObservation) {
        self.latched = self.latched.union(causes);
    }

    /// Every latched cause, masked or not.
    pub const fn latched(&self) -> MacPowerInterruptObservation {
        self.latched
    }

    /// The STATUS image: the latched causes whose source is enabled.
    pub const fn visible(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            false,
            false,
            false,
            false,
            self.sta_tbtt_enabled && self.latched.sta_tbtt(),
            false,
        )
    }

    pub const fn sta_tbtt_enabled(&self) -> bool {
        self.sta_tbtt_enabled
    }

    /// The level this bank drives towards its CPU interrupt line.
    pub const fn line_asserted(&self) -> bool {
        !self.visible().is_empty()
    }

    pub const fn operations(&self) -> &OperationLog<PowerInterruptOperation> {
        &self.operations
    }

    pub fn clear_operations(&mut self) {
        self.operations.clear();
    }
}

impl Default for PowerInterruptModel {
    fn default() -> Self {
        Self::new()
    }
}

impl sealed::Sealed for PowerInterruptModel {}
impl MacPowerInterrupt for PowerInterruptModel {
    type Snapshot = MacPowerInterruptObservation;

    fn status(&mut self) -> Self::Snapshot {
        self.operations.push(PowerInterruptOperation::Status);
        self.visible()
    }

    fn mask_all(&mut self) {
        self.operations.push(PowerInterruptOperation::MaskAll);
        self.sta_tbtt_enabled = false;
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.operations
            .push(PowerInterruptOperation::Acknowledge(snapshot));
        let latched = self.latched;
        self.latched = MacPowerInterruptObservation::from_semantic_events(
            latched.tsf_timer_0() && !snapshot.tsf_timer_0(),
            latched.tsf_timer_1() && !snapshot.tsf_timer_1(),
            latched.tsf_timer_2() && !snapshot.tsf_timer_2(),
            latched.tsf_timer_3() && !snapshot.tsf_timer_3(),
            latched.sta_tbtt() && !snapshot.sta_tbtt(),
            latched.has_unhandled_event() && !snapshot.has_unhandled_event(),
        );
    }

    fn clear_all(&mut self, _masked: &BanksMasked) {
        self.operations.push(PowerInterruptOperation::ClearAll);
        self.sta_tbtt_enabled = false;
        self.latched = MacPowerInterruptObservation::from_semantic_events(
            false, false, false, false, false, false,
        );
    }

    fn enable_station_tbtt(&mut self) {
        self.operations
            .push(PowerInterruptOperation::EnableStationTbtt);
        self.sta_tbtt_enabled = true;
    }
}
