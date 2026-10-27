//! Operations on generated BLE shared layouts under their CPU ownership protocol.

#![forbid(unsafe_code)]

use crate::layouts::*;

pub(crate) fn clear_link_state(layout: &BleLinkState) {
    layout.tx_path().write(BleLinkStateTxPath::ZERO);
    layout.scan_response().write(BleLinkStateScanResponse::ZERO);
    layout.rx_path().write(BleLinkStateRxPath::ZERO);
    layout
        .control_policy()
        .write(BleLinkStateControlPolicy::ZERO);
    layout
        .word_0010_opaque()
        .write(BleLinkStateWord0010Opaque::ZERO);
    layout.packet_policy().write(BleLinkStatePacketPolicy::ZERO);
    layout
        .receive_context()
        .write(BleLinkStateReceiveContext::ZERO);
    layout
        .word_001c_opaque()
        .write(BleLinkStateWord001cOpaque::ZERO);
    layout.rx_list().write(BleLinkStateRxList::ZERO);
    layout
        .own_address_policy()
        .write(BleLinkStateOwnAddressPolicy::ZERO);
    layout
        .word_0028_opaque()
        .write(BleLinkStateWord0028Opaque::ZERO);
    layout
        .crc_initialization()
        .write(BleLinkStateCrcInitialization::ZERO);
    layout
        .sequence_policy()
        .write(BleLinkStateSequencePolicy::ZERO);
    layout.event_span().write(BleLinkStateEventSpan::ZERO);
    layout
        .access_address()
        .write(BleLinkStateAccessAddress::ZERO);
    layout
        .own_address_low()
        .write(BleLinkStateOwnAddressLow::ZERO);
    layout
        .own_address_high()
        .write(BleLinkStateOwnAddressHigh::ZERO);
    layout
        .word_0044_opaque()
        .write(BleLinkStateWord0044Opaque::ZERO);
    layout
        .word_0048_opaque()
        .write(BleLinkStateWord0048Opaque::ZERO);
    layout
        .word_004c_opaque()
        .write(BleLinkStateWord004cOpaque::ZERO);
    layout
        .direction_finding()
        .write(BleLinkStateDirectionFinding::ZERO);
    layout
        .direction_finding_policy()
        .write(BleLinkStateDirectionFindingPolicy::ZERO);
    layout
        .word_0058_opaque()
        .write(BleLinkStateWord0058Opaque::ZERO);
    layout
        .word_005c_opaque()
        .write(BleLinkStateWord005cOpaque::ZERO);
    layout.event().write(BleLinkStateEvent::ZERO);
    layout
        .scheduler_head()
        .write(BleLinkStateSchedulerHead::ZERO);
    layout.rx_head().write(BleLinkStateRxHead::ZERO);
    layout.tx_head().write(BleLinkStateTxHead::ZERO);
    layout.rx_tail().write(BleLinkStateRxTail::ZERO);
    layout.tx_tail().write(BleLinkStateTxTail::ZERO);
    layout.rx_reserve().write(BleLinkStateRxReserve::ZERO);
    layout
        .rx_queue_configuration()
        .write(BleLinkStateRxQueueConfiguration::ZERO);
    layout
        .word_0080_opaque()
        .write(BleLinkStateWord0080Opaque::ZERO);
}

pub(crate) fn clear_scheduler_item(layout: &BleSchedulerItem) {
    layout
        .hardware_next()
        .write(BleSchedulerItemHardwareNext::ZERO);
    layout.context().write(BleSchedulerItemContext::ZERO);
    layout.link_state().write(BleSchedulerItemLinkState::ZERO);
    layout
        .sequence_start()
        .write(BleSchedulerItemSequenceStart::ZERO);
    layout
        .sequence_duration()
        .write(BleSchedulerItemSequenceDuration::ZERO);
    layout
        .rate_and_power()
        .write(BleSchedulerItemRateAndPower::ZERO);
    layout
        .channel_and_priority()
        .write(BleSchedulerItemChannelAndPriority::ZERO);
    layout
        .word_001c_opaque()
        .write(BleSchedulerItemWord001cOpaque::ZERO);
    layout
        .allocation_number()
        .write(BleSchedulerItemAllocationNumber::ZERO);
    layout
        .coexistence_priority()
        .write(BleSchedulerItemCoexistencePriority::ZERO);
    layout
        .word_0028_opaque()
        .write(BleSchedulerItemWord0028Opaque::ZERO);
    layout
        .receive_wait()
        .write(BleSchedulerItemReceiveWait::ZERO);
    layout.list_policy().write(BleSchedulerItemListPolicy::ZERO);
    layout
        .captured_anchor()
        .write(BleSchedulerItemCapturedAnchor::ZERO);
    layout.status().write(BleSchedulerItemStatus::ZERO);
    layout
        .word_003c_opaque()
        .write(BleSchedulerItemWord003cOpaque::ZERO);
    layout
        .word_0040_opaque()
        .write(BleSchedulerItemWord0040Opaque::ZERO);
    layout.raw_start().write(BleSchedulerItemRawStart::ZERO);
    layout.raw_end().write(BleSchedulerItemRawEnd::ZERO);
    layout.control().write(BleSchedulerItemControl::ZERO);
    layout.previous().write(BleSchedulerItemPrevious::ZERO);
    layout
        .completion_next()
        .write(BleSchedulerItemCompletionNext::ZERO);
    layout
        .recycle_callback()
        .write(BleSchedulerItemRecycleCallback::ZERO);
    layout
        .buffer_recycle_callback()
        .write(BleSchedulerItemBufferRecycleCallback::ZERO);
}

pub(crate) fn copy_buffer_header(layout: &BleBufferHeader, source: &BleBufferHeader) {
    layout.next().write(source.next().read());
    layout.packet_base().write(source.packet_base().read());
    layout.pdu_target().write(source.pdu_target().read());
    layout.completion().write(source.completion().read());
    layout.allocation().write(source.allocation().read());
    layout.previous().write(source.previous().read());
}

pub(crate) fn clear_direction_finding_workspace(layout: &BleDirectionFindingWorkspace) {
    layout
        .word_0000_opaque()
        .write(BleDirectionFindingWorkspaceWord0000Opaque::ZERO);
    layout
        .word_0004_opaque()
        .write(BleDirectionFindingWorkspaceWord0004Opaque::ZERO);
    layout
        .word_0008_opaque()
        .write(BleDirectionFindingWorkspaceWord0008Opaque::ZERO);
    layout
        .word_000c_opaque()
        .write(BleDirectionFindingWorkspaceWord000cOpaque::ZERO);
    layout
        .word_0010_opaque()
        .write(BleDirectionFindingWorkspaceWord0010Opaque::ZERO);
    layout
        .word_0014_opaque()
        .write(BleDirectionFindingWorkspaceWord0014Opaque::ZERO);
    layout
        .word_0018_opaque()
        .write(BleDirectionFindingWorkspaceWord0018Opaque::ZERO);
    layout
        .word_001c_opaque()
        .write(BleDirectionFindingWorkspaceWord001cOpaque::ZERO);
}

pub(crate) fn clear_resolving_list(layout: &BleResolvingList) {
    layout
        .word_0000_opaque()
        .write(BleResolvingListWord0000Opaque::ZERO);
    layout
        .word_0004_opaque()
        .write(BleResolvingListWord0004Opaque::ZERO);
    layout
        .word_0008_opaque()
        .write(BleResolvingListWord0008Opaque::ZERO);
    layout
        .word_000c_opaque()
        .write(BleResolvingListWord000cOpaque::ZERO);
    layout
        .word_0010_opaque()
        .write(BleResolvingListWord0010Opaque::ZERO);
    layout
        .word_0014_opaque()
        .write(BleResolvingListWord0014Opaque::ZERO);
    layout
        .word_0018_opaque()
        .write(BleResolvingListWord0018Opaque::ZERO);
    layout
        .word_001c_opaque()
        .write(BleResolvingListWord001cOpaque::ZERO);
    layout
        .word_0020_opaque()
        .write(BleResolvingListWord0020Opaque::ZERO);
    layout
        .word_0024_opaque()
        .write(BleResolvingListWord0024Opaque::ZERO);
    layout
        .word_0028_opaque()
        .write(BleResolvingListWord0028Opaque::ZERO);
    layout
        .word_002c_opaque()
        .write(BleResolvingListWord002cOpaque::ZERO);
    layout
        .word_0030_opaque()
        .write(BleResolvingListWord0030Opaque::ZERO);
    layout
        .word_0034_opaque()
        .write(BleResolvingListWord0034Opaque::ZERO);
    layout
        .word_0038_opaque()
        .write(BleResolvingListWord0038Opaque::ZERO);
    layout
        .word_003c_opaque()
        .write(BleResolvingListWord003cOpaque::ZERO);
}

pub(crate) fn clear_phy_environment(layout: &BlePhyEnvironment) {
    layout
        .word_0000_opaque()
        .write(BlePhyEnvironmentWord0000Opaque::ZERO);
    layout
        .word_0004_opaque()
        .write(BlePhyEnvironmentWord0004Opaque::ZERO);
    layout
        .word_0008_opaque()
        .write(BlePhyEnvironmentWord0008Opaque::ZERO);
    layout
        .word_000c_opaque()
        .write(BlePhyEnvironmentWord000cOpaque::ZERO);
    layout
        .word_0010_opaque()
        .write(BlePhyEnvironmentWord0010Opaque::ZERO);
    layout
        .word_0014_opaque()
        .write(BlePhyEnvironmentWord0014Opaque::ZERO);
    layout
        .word_0018_opaque()
        .write(BlePhyEnvironmentWord0018Opaque::ZERO);
    layout
        .word_001c_opaque()
        .write(BlePhyEnvironmentWord001cOpaque::ZERO);
    layout
        .word_0020_opaque()
        .write(BlePhyEnvironmentWord0020Opaque::ZERO);
    layout
        .word_0024_opaque()
        .write(BlePhyEnvironmentWord0024Opaque::ZERO);
    layout
        .word_0028_opaque()
        .write(BlePhyEnvironmentWord0028Opaque::ZERO);
    layout
        .word_002c_opaque()
        .write(BlePhyEnvironmentWord002cOpaque::ZERO);
    layout
        .channel_frequency_offsets()
        .write(BlePhyEnvironmentChannelFrequencyOffsets::ZERO);
    layout
        .packet_start_offsets()
        .write(BlePhyEnvironmentPacketStartOffsets::ZERO);
    layout
        .rx_address_delays()
        .write(BlePhyEnvironmentRxAddressDelays::ZERO);
    layout
        .word_003c_opaque()
        .write(BlePhyEnvironmentWord003cOpaque::ZERO);
    layout
        .word_0040_opaque()
        .write(BlePhyEnvironmentWord0040Opaque::ZERO);
    layout
        .word_0044_opaque()
        .write(BlePhyEnvironmentWord0044Opaque::ZERO);
    layout
        .word_0048_opaque()
        .write(BlePhyEnvironmentWord0048Opaque::ZERO);
    layout
        .tx_power_levels()
        .write(BlePhyEnvironmentTxPowerLevels::ZERO);
    layout
        .tx_power_level_count()
        .write(BlePhyEnvironmentTxPowerLevelCount::ZERO);
    layout
        .word_0054_opaque()
        .write(BlePhyEnvironmentWord0054Opaque::ZERO);
    layout
        .word_0058_opaque()
        .write(BlePhyEnvironmentWord0058Opaque::ZERO);
    layout
        .word_005c_opaque()
        .write(BlePhyEnvironmentWord005cOpaque::ZERO);
    layout
        .word_0060_opaque()
        .write(BlePhyEnvironmentWord0060Opaque::ZERO);
    layout
        .word_0064_opaque()
        .write(BlePhyEnvironmentWord0064Opaque::ZERO);
    layout
        .word_0068_opaque()
        .write(BlePhyEnvironmentWord0068Opaque::ZERO);
    layout
        .word_006c_opaque()
        .write(BlePhyEnvironmentWord006cOpaque::ZERO);
    layout
        .word_0070_opaque()
        .write(BlePhyEnvironmentWord0070Opaque::ZERO);
    layout
        .word_0074_opaque()
        .write(BlePhyEnvironmentWord0074Opaque::ZERO);
    layout
        .word_0078_opaque()
        .write(BlePhyEnvironmentWord0078Opaque::ZERO);
    layout
        .tx_power_first_level()
        .write(BlePhyEnvironmentTxPowerFirstLevel::ZERO);
    layout
        .tx_power_last_level()
        .write(BlePhyEnvironmentTxPowerLastLevel::ZERO);
}

pub(crate) fn clear_scheduler_context(layout: &BleSchedulerContext) {
    layout
        .word_0000_opaque()
        .write(BleSchedulerContextWord0000Opaque::ZERO);
    layout
        .word_0004_opaque()
        .write(BleSchedulerContextWord0004Opaque::ZERO);
    layout
        .word_0008_opaque()
        .write(BleSchedulerContextWord0008Opaque::ZERO);
    layout
        .word_000c_opaque()
        .write(BleSchedulerContextWord000cOpaque::ZERO);
    layout
        .word_0010_opaque()
        .write(BleSchedulerContextWord0010Opaque::ZERO);
    layout
        .word_0014_opaque()
        .write(BleSchedulerContextWord0014Opaque::ZERO);
    layout
        .word_0018_opaque()
        .write(BleSchedulerContextWord0018Opaque::ZERO);
    layout
        .word_001c_opaque()
        .write(BleSchedulerContextWord001cOpaque::ZERO);
    layout
        .word_0020_opaque()
        .write(BleSchedulerContextWord0020Opaque::ZERO);
    layout
        .word_0024_opaque()
        .write(BleSchedulerContextWord0024Opaque::ZERO);
    layout
        .word_0028_opaque()
        .write(BleSchedulerContextWord0028Opaque::ZERO);
    layout
        .word_002c_opaque()
        .write(BleSchedulerContextWord002cOpaque::ZERO);
    layout
        .word_0030_opaque()
        .write(BleSchedulerContextWord0030Opaque::ZERO);
    layout
        .word_0034_opaque()
        .write(BleSchedulerContextWord0034Opaque::ZERO);
    layout
        .word_0038_opaque()
        .write(BleSchedulerContextWord0038Opaque::ZERO);
    layout
        .word_003c_opaque()
        .write(BleSchedulerContextWord003cOpaque::ZERO);
    layout
        .word_0040_opaque()
        .write(BleSchedulerContextWord0040Opaque::ZERO);
    layout
        .word_0044_opaque()
        .write(BleSchedulerContextWord0044Opaque::ZERO);
}
/// CPU value of the complete scanner link state, with generated word values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinkStateImage {
    pub(crate) tx_path: BleLinkStateTxPath,
    pub(crate) scan_response: BleLinkStateScanResponse,
    pub(crate) rx_path: BleLinkStateRxPath,
    pub(crate) control_policy: BleLinkStateControlPolicy,
    pub(crate) word_0010_opaque: BleLinkStateWord0010Opaque,
    pub(crate) packet_policy: BleLinkStatePacketPolicy,
    pub(crate) receive_context: BleLinkStateReceiveContext,
    pub(crate) word_001c_opaque: BleLinkStateWord001cOpaque,
    pub(crate) rx_list: BleLinkStateRxList,
    pub(crate) own_address_policy: BleLinkStateOwnAddressPolicy,
    pub(crate) word_0028_opaque: BleLinkStateWord0028Opaque,
    pub(crate) crc_initialization: BleLinkStateCrcInitialization,
    pub(crate) sequence_policy: BleLinkStateSequencePolicy,
    pub(crate) event_span: BleLinkStateEventSpan,
    pub(crate) access_address: BleLinkStateAccessAddress,
    pub(crate) own_address_low: BleLinkStateOwnAddressLow,
    pub(crate) own_address_high: BleLinkStateOwnAddressHigh,
    pub(crate) word_0044_opaque: BleLinkStateWord0044Opaque,
    pub(crate) word_0048_opaque: BleLinkStateWord0048Opaque,
    pub(crate) word_004c_opaque: BleLinkStateWord004cOpaque,
    pub(crate) direction_finding: BleLinkStateDirectionFinding,
    pub(crate) direction_finding_policy: BleLinkStateDirectionFindingPolicy,
    pub(crate) word_0058_opaque: BleLinkStateWord0058Opaque,
    pub(crate) word_005c_opaque: BleLinkStateWord005cOpaque,
    pub(crate) event: BleLinkStateEvent,
    pub(crate) scheduler_head: BleLinkStateSchedulerHead,
    pub(crate) rx_head: BleLinkStateRxHead,
    pub(crate) tx_head: BleLinkStateTxHead,
    pub(crate) rx_tail: BleLinkStateRxTail,
    pub(crate) tx_tail: BleLinkStateTxTail,
    pub(crate) rx_reserve: BleLinkStateRxReserve,
    pub(crate) rx_queue_configuration: BleLinkStateRxQueueConfiguration,
    pub(crate) word_0080_opaque: BleLinkStateWord0080Opaque,
}

impl LinkStateImage {
    pub(crate) const ZERO: Self = Self {
        tx_path: BleLinkStateTxPath::ZERO,
        scan_response: BleLinkStateScanResponse::ZERO,
        rx_path: BleLinkStateRxPath::ZERO,
        control_policy: BleLinkStateControlPolicy::ZERO,
        word_0010_opaque: BleLinkStateWord0010Opaque::ZERO,
        packet_policy: BleLinkStatePacketPolicy::ZERO,
        receive_context: BleLinkStateReceiveContext::ZERO,
        word_001c_opaque: BleLinkStateWord001cOpaque::ZERO,
        rx_list: BleLinkStateRxList::ZERO,
        own_address_policy: BleLinkStateOwnAddressPolicy::ZERO,
        word_0028_opaque: BleLinkStateWord0028Opaque::ZERO,
        crc_initialization: BleLinkStateCrcInitialization::ZERO,
        sequence_policy: BleLinkStateSequencePolicy::ZERO,
        event_span: BleLinkStateEventSpan::ZERO,
        access_address: BleLinkStateAccessAddress::ZERO,
        own_address_low: BleLinkStateOwnAddressLow::ZERO,
        own_address_high: BleLinkStateOwnAddressHigh::ZERO,
        word_0044_opaque: BleLinkStateWord0044Opaque::ZERO,
        word_0048_opaque: BleLinkStateWord0048Opaque::ZERO,
        word_004c_opaque: BleLinkStateWord004cOpaque::ZERO,
        direction_finding: BleLinkStateDirectionFinding::ZERO,
        direction_finding_policy: BleLinkStateDirectionFindingPolicy::ZERO,
        word_0058_opaque: BleLinkStateWord0058Opaque::ZERO,
        word_005c_opaque: BleLinkStateWord005cOpaque::ZERO,
        event: BleLinkStateEvent::ZERO,
        scheduler_head: BleLinkStateSchedulerHead::ZERO,
        rx_head: BleLinkStateRxHead::ZERO,
        tx_head: BleLinkStateTxHead::ZERO,
        rx_tail: BleLinkStateRxTail::ZERO,
        tx_tail: BleLinkStateTxTail::ZERO,
        rx_reserve: BleLinkStateRxReserve::ZERO,
        rx_queue_configuration: BleLinkStateRxQueueConfiguration::ZERO,
        word_0080_opaque: BleLinkStateWord0080Opaque::ZERO,
    };

    pub(crate) fn read(layout: &BleLinkState) -> Self {
        Self {
            tx_path: layout.tx_path().read(),
            scan_response: layout.scan_response().read(),
            rx_path: layout.rx_path().read(),
            control_policy: layout.control_policy().read(),
            word_0010_opaque: layout.word_0010_opaque().read(),
            packet_policy: layout.packet_policy().read(),
            receive_context: layout.receive_context().read(),
            word_001c_opaque: layout.word_001c_opaque().read(),
            rx_list: layout.rx_list().read(),
            own_address_policy: layout.own_address_policy().read(),
            word_0028_opaque: layout.word_0028_opaque().read(),
            crc_initialization: layout.crc_initialization().read(),
            sequence_policy: layout.sequence_policy().read(),
            event_span: layout.event_span().read(),
            access_address: layout.access_address().read(),
            own_address_low: layout.own_address_low().read(),
            own_address_high: layout.own_address_high().read(),
            word_0044_opaque: layout.word_0044_opaque().read(),
            word_0048_opaque: layout.word_0048_opaque().read(),
            word_004c_opaque: layout.word_004c_opaque().read(),
            direction_finding: layout.direction_finding().read(),
            direction_finding_policy: layout.direction_finding_policy().read(),
            word_0058_opaque: layout.word_0058_opaque().read(),
            word_005c_opaque: layout.word_005c_opaque().read(),
            event: layout.event().read(),
            scheduler_head: layout.scheduler_head().read(),
            rx_head: layout.rx_head().read(),
            tx_head: layout.tx_head().read(),
            rx_tail: layout.rx_tail().read(),
            tx_tail: layout.tx_tail().read(),
            rx_reserve: layout.rx_reserve().read(),
            rx_queue_configuration: layout.rx_queue_configuration().read(),
            word_0080_opaque: layout.word_0080_opaque().read(),
        }
    }

    pub(crate) fn write(self, layout: &BleLinkState) {
        layout.tx_path().write(self.tx_path);
        layout.scan_response().write(self.scan_response);
        layout.rx_path().write(self.rx_path);
        layout.control_policy().write(self.control_policy);
        layout.word_0010_opaque().write(self.word_0010_opaque);
        layout.packet_policy().write(self.packet_policy);
        layout.receive_context().write(self.receive_context);
        layout.word_001c_opaque().write(self.word_001c_opaque);
        layout.rx_list().write(self.rx_list);
        layout.own_address_policy().write(self.own_address_policy);
        layout.word_0028_opaque().write(self.word_0028_opaque);
        layout.crc_initialization().write(self.crc_initialization);
        layout.sequence_policy().write(self.sequence_policy);
        layout.event_span().write(self.event_span);
        layout.access_address().write(self.access_address);
        layout.own_address_low().write(self.own_address_low);
        layout.own_address_high().write(self.own_address_high);
        layout.word_0044_opaque().write(self.word_0044_opaque);
        layout.word_0048_opaque().write(self.word_0048_opaque);
        layout.word_004c_opaque().write(self.word_004c_opaque);
        layout.direction_finding().write(self.direction_finding);
        layout
            .direction_finding_policy()
            .write(self.direction_finding_policy);
        layout.word_0058_opaque().write(self.word_0058_opaque);
        layout.word_005c_opaque().write(self.word_005c_opaque);
        layout.event().write(self.event);
        layout.scheduler_head().write(self.scheduler_head);
        layout.rx_head().write(self.rx_head);
        layout.tx_head().write(self.tx_head);
        layout.rx_tail().write(self.rx_tail);
        layout.tx_tail().write(self.tx_tail);
        layout.rx_reserve().write(self.rx_reserve);
        layout
            .rx_queue_configuration()
            .write(self.rx_queue_configuration);
        layout.word_0080_opaque().write(self.word_0080_opaque);
    }
}
/// Value of a buffer header while its CPU owner installs or rotates it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BufferHeaderImage {
    pub(crate) next: BleBufferHeaderNext,
    pub(crate) packet_base: BleBufferHeaderPacketBase,
    pub(crate) pdu_target: BleBufferHeaderPduTarget,
    pub(crate) completion: BleBufferHeaderCompletion,
    pub(crate) allocation: BleBufferHeaderAllocation,
    pub(crate) previous: BleBufferHeaderPrevious,
}
impl BufferHeaderImage {
    pub(crate) const ZERO: Self = Self {
        next: BleBufferHeaderNext::ZERO,
        packet_base: BleBufferHeaderPacketBase::ZERO,
        pdu_target: BleBufferHeaderPduTarget::ZERO,
        completion: BleBufferHeaderCompletion::ZERO,
        allocation: BleBufferHeaderAllocation::ZERO,
        previous: BleBufferHeaderPrevious::ZERO,
    };
    #[cfg(test)]
    pub(crate) fn read(layout: &BleBufferHeader) -> Self {
        Self {
            next: layout.next().read(),
            packet_base: layout.packet_base().read(),
            pdu_target: layout.pdu_target().read(),
            completion: layout.completion().read(),
            allocation: layout.allocation().read(),
            previous: layout.previous().read(),
        }
    }
    pub(crate) fn write(self, layout: &BleBufferHeader) {
        layout.next().write(self.next);
        layout.packet_base().write(self.packet_base);
        layout.pdu_target().write(self.pdu_target);
        layout.completion().write(self.completion);
        layout.allocation().write(self.allocation);
        layout.previous().write(self.previous);
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SchedulerItemImage {
    hardware_next: BleSchedulerItemHardwareNext,
    context: BleSchedulerItemContext,
    link_state: BleSchedulerItemLinkState,
    sequence_start: BleSchedulerItemSequenceStart,
    sequence_duration: BleSchedulerItemSequenceDuration,
    rate_and_power: BleSchedulerItemRateAndPower,
    channel_and_priority: BleSchedulerItemChannelAndPriority,
    word_001c_opaque: BleSchedulerItemWord001cOpaque,
    allocation_number: BleSchedulerItemAllocationNumber,
    coexistence_priority: BleSchedulerItemCoexistencePriority,
    word_0028_opaque: BleSchedulerItemWord0028Opaque,
    receive_wait: BleSchedulerItemReceiveWait,
    list_policy: BleSchedulerItemListPolicy,
    captured_anchor: BleSchedulerItemCapturedAnchor,
    status: BleSchedulerItemStatus,
    word_003c_opaque: BleSchedulerItemWord003cOpaque,
    word_0040_opaque: BleSchedulerItemWord0040Opaque,
    raw_start: BleSchedulerItemRawStart,
    raw_end: BleSchedulerItemRawEnd,
    control: BleSchedulerItemControl,
    previous: BleSchedulerItemPrevious,
    completion_next: BleSchedulerItemCompletionNext,
    recycle_callback: BleSchedulerItemRecycleCallback,
    buffer_recycle_callback: BleSchedulerItemBufferRecycleCallback,
}
#[cfg(test)]
impl SchedulerItemImage {
    pub(crate) fn read(layout: &BleSchedulerItem) -> Self {
        Self {
            hardware_next: layout.hardware_next().read(),
            context: layout.context().read(),
            link_state: layout.link_state().read(),
            sequence_start: layout.sequence_start().read(),
            sequence_duration: layout.sequence_duration().read(),
            rate_and_power: layout.rate_and_power().read(),
            channel_and_priority: layout.channel_and_priority().read(),
            word_001c_opaque: layout.word_001c_opaque().read(),
            allocation_number: layout.allocation_number().read(),
            coexistence_priority: layout.coexistence_priority().read(),
            word_0028_opaque: layout.word_0028_opaque().read(),
            receive_wait: layout.receive_wait().read(),
            list_policy: layout.list_policy().read(),
            captured_anchor: layout.captured_anchor().read(),
            status: layout.status().read(),
            word_003c_opaque: layout.word_003c_opaque().read(),
            word_0040_opaque: layout.word_0040_opaque().read(),
            raw_start: layout.raw_start().read(),
            raw_end: layout.raw_end().read(),
            control: layout.control().read(),
            previous: layout.previous().read(),
            completion_next: layout.completion_next().read(),
            recycle_callback: layout.recycle_callback().read(),
            buffer_recycle_callback: layout.buffer_recycle_callback().read(),
        }
    }
}
