//! Role value builders and list selection. None borrows controller layouts.

mod dtm_event_image;
mod legacy_scanning_event_image;
pub use legacy_scanning_event_image::{
    LegacyScanFilterPolicy, LegacyScanResetConfig, LegacyScanStartSelection, LegacyScanType,
};
mod legacy_advertising_event_image;
pub use dtm_event_image::{
    DtmReceiverEventPhase, DtmRole, DtmSchedulerItemEventType, DtmSchedulerReceiverPhy,
    DtmSchedulerTransmitterPhy,
};
pub use legacy_advertising_event_image::LegacyAdvertisingPrimaryChannelPlan;

use oer_esp32s31_hal::bluetooth::engine::scheduler::SchedulerRawWindow;
use oer_esp32s31_hal::bluetooth::{BluetoothSchedulerHardwareListIndex, engine::*};

/// The current hardware engine joins all supported roles on list zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    DirectTestMode,
    LegacyAdvertising,
    ConnectableAdvertising,
    LegacyScanning,
    PeripheralConnection,
}

impl Role {
    pub const fn list(self) -> BluetoothSchedulerHardwareListIndex {
        match self {
            Self::DirectTestMode
            | Self::LegacyAdvertising
            | Self::ConnectableAdvertising
            | Self::LegacyScanning
            | Self::PeripheralConnection => BluetoothSchedulerHardwareListIndex::ZERO,
        }
    }
}

/// Initial reset versus a recurring event that retains the live session image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmEventPhase {
    Initial,
    Recurring,
}

/// An owned recipe, rather than a CPU view of a controller-owned descriptor.
#[derive(Clone, Copy, Debug)]
pub struct DtmImageBuilder {
    role: DtmRole,
    event: DtmSchedulerItemEventType,
    phase: DtmEventPhase,
    power: LeTxPower,
    configuration: u8,
    frequency: DtmSchedulerFrequency,
    window: SchedulerRawWindow,
    sequence_start: u32,
}

impl DtmImageBuilder {
    pub const fn transmitter(
        phase: DtmEventPhase,
        phy: DtmSchedulerTransmitterPhy,
        power: LeTxPower,
        configuration: u8,
        frequency: DtmSchedulerFrequency,
        window: SchedulerRawWindow,
        sequence_start: u32,
    ) -> Self {
        Self {
            role: DtmRole::Transmitter,
            event: DtmSchedulerItemEventType::Transmitter(phy),
            phase,
            power,
            configuration,
            frequency,
            window,
            sequence_start,
        }
    }

    pub const fn receiver(
        phase: DtmEventPhase,
        phy: DtmSchedulerReceiverPhy,
        power: LeTxPower,
        configuration: u8,
        frequency: DtmSchedulerFrequency,
        window: SchedulerRawWindow,
        sequence_start: u32,
    ) -> Self {
        let receiver_phase = match phase {
            DtmEventPhase::Initial => DtmReceiverEventPhase::Initial,
            DtmEventPhase::Recurring => DtmReceiverEventPhase::Recurring,
        };
        Self {
            role: DtmRole::Receiver,
            event: DtmSchedulerItemEventType::Receiver {
                phy,
                phase: receiver_phase,
            },
            phase,
            power,
            configuration,
            frequency,
            window,
            sequence_start,
        }
    }

    /// HAL supplies a detached value seed and retains the graph. Its commit
    /// separately verifies every bound pointer before changing live memory.
    pub fn build(self, seed: DtmPositionalEventSeed) -> DtmPositionalEventWords {
        let words = seed.words();
        let link_state = match self.phase {
            DtmEventPhase::Initial => words.link_state().apply_reset(
                Some(seed.tx_header_head_projection()),
                Some(seed.rx_header_tail_projection()),
                self.power,
                self.configuration,
                self.role == DtmRole::Receiver,
            ),
            DtmEventPhase::Recurring => words.link_state(),
        };
        let item = words
            .scheduler_item()
            .apply_event(
                self.frequency,
                self.event.configuration(),
                self.window.start(),
                self.window.end(),
            )
            .apply_sequence_start(self.sequence_start);
        let item = match self.phase {
            DtmEventPhase::Initial => item,
            DtmEventPhase::Recurring => item.apply_recurring_power(link_state),
        };
        DtmPositionalEventWords::new(link_state, item)
    }

    pub fn prepare<'pool, 'epoch, const N: usize>(
        self,
        pool: &mut DtmPool<'pool, 'epoch, N>,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<(), DtmPrepareError> {
        pool.prepare_event(lease, |seed| Ok(self.build(seed)))
    }
}

/// The advertiser chooses the PDU, primary-channel plan, timing and PTI;
/// HAL alone installs links and commits the hardware words.
pub struct AdvertisingImageBuilder<'pdu> {
    pub pdu: &'pdu [u8],
    pub power: LeTxPower,
    pub channels: LegacyAdvertisingPrimaryChannelPlan,
    pub start: u32,
    pub item_duration: u32,
    pub coexistence: AdvertisingCoexistencePriorities,
}

impl AdvertisingImageBuilder<'_> {
    pub fn prepare<'pool, 'epoch, const N: usize>(
        self,
        pool: &mut LegacyAdvertisingPool<'pool, 'epoch, N>,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<LegacyAdvertisingEvent, LegacyAdvertisingError> {
        let address = legacy_advertising_event_image::own_address_from_pdu(self.pdu)
            .map_err(LegacyAdvertisingError::Pdu)?;
        pool.prepare_packet(lease, self.pdu)?;
        pool.reset_link_state(lease, self.power, address, ROLE_PRIORITY)?;
        pool.prepare_event(
            lease,
            &self.channels.image(self.start, self.item_duration, 0),
            self.coexistence,
        )
    }
}

pub struct ScanningImageBuilder {
    pub reset: LegacyScanResetConfig,
    pub channel: LegacyScanPrimaryChannel,
    pub timing: LegacyScanEventTiming,
    pub start: LegacyScanStartSelection,
    pub coexistence: LegacyScanCoexistencePriorities,
}

impl ScanningImageBuilder {
    pub fn prepare<
        'pool,
        'epoch,
        const A: usize,
        const C: usize,
        const N: usize,
        const R: usize,
        const O: usize,
        const Q: usize,
    >(
        self,
        engine: &mut Engine<ArenaMemory<'pool, 'epoch, A, C, N, R, O, Q>>,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<LegacyScanEvent, EnginePrepareError<LegacyScanError>> {
        engine.reset_scanning(lease, self.reset.image())?;
        engine
            .scanning()
            .map_err(|_| EnginePrepareError::Quarantined)?
            .prepare_event(
                lease,
                self.start.image(self.channel),
                self.timing,
                self.coexistence,
            )
            .map_err(EnginePrepareError::Role)
    }
}

pub struct ConnectableAdvertisingImageBuilder<'pdu> {
    pub packets: LegacyConnectableAdvertisingMemoryInput<'pdu>,
    pub power: LeTxPower,
    pub channels: LegacyAdvertisingPrimaryChannelPlan,
    pub start: u32,
    pub item_duration: u32,
    pub sequence_lead: u32,
    pub coexistence: AdvertisingCoexistencePriorities,
}

impl ConnectableAdvertisingImageBuilder<'_> {
    pub fn prepare<
        'pool,
        'epoch,
        const A: usize,
        const C: usize,
        const N: usize,
        const R: usize,
        const O: usize,
        const Q: usize,
    >(
        self,
        engine: &mut Engine<ArenaMemory<'pool, 'epoch, A, C, N, R, O, Q>>,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<LegacyAdvertisingEvent, EnginePrepareError<LegacyConnectableAdvertisingError>> {
        engine.prepare_connectable_advertising(lease, self.packets, self.power, ROLE_PRIORITY)?;
        engine
            .connectable_advertising()
            .map_err(|_| EnginePrepareError::Quarantined)?
            .prepare_event(
                lease,
                &self
                    .channels
                    .image(self.start, self.item_duration, self.sequence_lead),
                self.coexistence,
            )
            .map_err(EnginePrepareError::Role)
    }
}

pub struct PeripheralConnectionImageBuilder {
    pub identity: PeripheralConnectionIdentity,
    pub event: PeripheralConnectionFirstEvent,
}

impl PeripheralConnectionImageBuilder {
    pub fn prepare<
        'pool,
        'epoch,
        const A: usize,
        const C: usize,
        const N: usize,
        const R: usize,
        const O: usize,
        const Q: usize,
    >(
        self,
        engine: &mut Engine<ArenaMemory<'pool, 'epoch, A, C, N, R, O, Q>>,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<PeripheralConnectionEvent, EnginePrepareError<PeripheralConnectionError>> {
        engine
            .connections()
            .map_err(|_| EnginePrepareError::Quarantined)?
            .prepare_identity(lease, self.identity)
            .map_err(EnginePrepareError::Role)?;
        engine.prepare_first_connection(lease, self.event)
    }
}

#[cfg(test)]
mod tests;

const ROLE_PRIORITY: SchedulerEventPriority =
    SchedulerEventPriority::new(1).expect("reviewed priority fits");
