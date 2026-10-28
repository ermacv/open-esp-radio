//! The engine and its MAC backend held as one affine owner.
//!
//! Entry points borrow the stored MAC internally. A chip HAL constructs this
//! owner from its placed buffers and exposes only these semantic operations.

use super::*;
use core::mem::ManuallyDrop;

/// Absorbing quarantine: the MAC and frame owners cannot be recovered or used.
/// Dropping this value retains both owners until reset.
///
/// ```compile_fail,E0599
/// use oer_espressif_ieee802154_engine::{engine::owner::Quarantined, ll::Ieee802154LowLevel};
/// fn release<L: Ieee802154LowLevel>(fault: Quarantined<'_, L>) {
///     fault.detach();
/// }
/// ```
#[must_use = "record the fault and reset with the owners quarantined"]
pub struct Quarantined<'storage, Ll: Ieee802154LowLevel> {
    fault: Fault,
    _owners: ManuallyDrop<Ieee802154EngineOwner<'storage, Ll>>,
}

impl<Ll: Ieee802154LowLevel> Quarantined<'_, Ll> {
    /// Source observations only; this grants no owner or register access.
    pub const fn fault(&self) -> Fault {
        self.fault
    }
}

impl<Ll: Ieee802154LowLevel> core::fmt::Debug for Quarantined<'_, Ll> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.fault.fmt(f)
    }
}

/// Quiescent engine retaining its unique frame storage while its MAC is absent.
/// No operation or storage access is available until it is rebound.
#[must_use = "retain the detached engine to reuse its frame storage"]
pub struct DetachedIeee802154Engine<'storage, Address: Copy + Eq> {
    engine: Ieee802154Engine<'storage, Address>,
}

impl<'storage, Address: Copy + Eq> DetachedIeee802154Engine<'storage, Address> {
    /// Reunite the retained engine and storage with its MAC for another epoch.
    /// This performs no register access; initialize the MAC before starting RF.
    pub fn rebind<Ll: Ieee802154LowLevel<Address = Address>>(
        self,
        ll: Ll,
    ) -> Ieee802154EngineOwner<'storage, Ll> {
        Ieee802154EngineOwner::new(self.engine, ll)
    }
}

/// One MAC backend together with the engine that owns its frame buffers.
///
/// Neither the backend nor the storage can be extracted while the owner is
/// live. Forgetting or dropping it retains the static storage's claim.
#[must_use = "retain the MAC with its frame storage for the hardware epoch"]
pub struct Ieee802154EngineOwner<'storage, Ll: Ieee802154LowLevel> {
    engine: Ieee802154Engine<'storage, Ll::Address>,
    ll: Ll,
}

impl<'storage, Ll: Ieee802154LowLevel> Ieee802154EngineOwner<'storage, Ll> {
    /// Hold the MAC together with the engine built from its placed buffers.
    pub fn new(engine: Ieee802154Engine<'storage, Ll::Address>, ll: Ll) -> Self {
        Self { engine, ll }
    }

    /// Return the MAC only after the engine's operation/DMA epoch has ended
    /// and every delivered receive slot has been returned. Stop the operation
    /// and silence the platform IRQ route before detaching. A software-only
    /// `disable` does not establish quiescence. No register access occurs here.
    ///
    /// # Errors
    /// Returns this complete owner unchanged if it is still active.
    #[allow(
        clippy::result_large_err,
        clippy::type_complexity,
        reason = "failure retains the affine owner and its unique storage"
    )]
    pub fn detach(self) -> Result<(DetachedIeee802154Engine<'storage, Ll::Address>, Ll), Self> {
        if !self.engine.quiescent() {
            return Err(self);
        }
        let Self { engine, ll } = self;
        Ok((DetachedIeee802154Engine { engine }, ll))
    }

    /// The recording MAC, for a host model to latch events and inspect publications.
    #[cfg(all(not(target_arch = "riscv32"), any(test, feature = "test-support")))]
    pub fn model_ll(&mut self) -> &mut Ll {
        &mut self.ll
    }

    /// [`Ieee802154Engine::set_coexistence`], using this owner's MAC.
    pub fn set_coexistence(&mut self, coexistence: Ieee802154Coexistence) {
        self.engine.set_coexistence(coexistence)
    }

    /// [`Ieee802154Engine::pib_defaults`], using this owner's MAC.
    pub const fn pib_defaults(&self) -> Ieee802154PibDefaults {
        self.engine.pib_defaults()
    }

    /// [`Ieee802154Engine::tx_power_levels`], using this owner's MAC.
    pub const fn tx_power_levels(&self) -> Ieee802154TxPowerLevels<'static> {
        self.engine.tx_power_levels()
    }

    /// [`Ieee802154Engine::coexistence`], using this owner's MAC.
    pub const fn coexistence(&self) -> Ieee802154Coexistence {
        self.engine.coexistence()
    }

    /// [`Ieee802154Engine::multipan`], using this owner's MAC.
    pub const fn multipan(&self) -> Option<Ieee802154Interfaces> {
        self.engine.multipan()
    }

    /// [`Ieee802154Engine::state`], using this owner's MAC.
    pub const fn state(&self) -> Ieee802154State {
        self.engine.state()
    }

    /// [`Ieee802154Engine::pib`], using this owner's MAC.
    pub fn pib(&self) -> &Ieee802154Pib {
        self.engine.pib()
    }

    /// [`Ieee802154Engine::update_pib`], applying RX changes in this entry.
    pub fn update_pib<E: Ieee802154Environment + ?Sized>(
        &mut self,
        env: &mut E,
        update: impl FnOnce(&mut Ieee802154Pib),
    ) {
        self.engine.update_pib(&mut self.ll, env, update)
    }

    /// [`Ieee802154Engine::set_pending_mode`], applying at the safe RX point.
    pub fn set_pending_mode<E: Ieee802154Environment + ?Sized>(
        &mut self,
        env: &mut E,
        index: Ieee802154MultipanIndex,
        mode: oer_ieee802154_mac::AutoPendingMode,
    ) {
        self.engine.set_pending_mode(&mut self.ll, env, index, mode)
    }

    /// [`Ieee802154Engine::pending_table`], using this owner's MAC.
    pub fn pending_table(&mut self) -> &mut PendingTable<PENDING_TABLE_SIZE> {
        self.engine.pending_table()
    }

    /// [`Ieee802154Engine::pending_table_for`], using this owner's MAC.
    pub fn pending_table_for(
        &mut self,
        index: Ieee802154MultipanIndex,
    ) -> &mut PendingTable<PENDING_TABLE_SIZE> {
        self.engine.pending_table_for(index)
    }

    /// [`Ieee802154Engine::set_multipan_panid`], using this owner's MAC.
    pub fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.engine.set_multipan_panid(&mut self.ll, index, panid)
    }

    /// [`Ieee802154Engine::multipan_panid`], using this owner's MAC.
    pub fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.engine.multipan_panid(&mut self.ll, index)
    }

    /// [`Ieee802154Engine::set_multipan_short_address`], using this owner's MAC.
    pub fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.engine
            .set_multipan_short_address(&mut self.ll, index, address)
    }

    /// [`Ieee802154Engine::multipan_short_address`], using this owner's MAC.
    pub fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.engine.multipan_short_address(&mut self.ll, index)
    }

    /// [`Ieee802154Engine::set_multipan_extended_address`], using this owner's MAC.
    pub fn set_multipan_extended_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: [u8; 8],
    ) {
        self.engine
            .set_multipan_extended_address(&mut self.ll, index, address)
    }

    /// [`Ieee802154Engine::multipan_extended_address`], using this owner's MAC.
    pub fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.engine.multipan_extended_address(&mut self.ll, index)
    }

    /// [`Ieee802154Engine::set_multipan_enable`], using this owner's MAC.
    pub fn set_multipan_enable(&mut self, enable: Ieee802154MultipanEnableState) {
        self.engine.set_multipan_enable(&mut self.ll, enable)
    }

    /// [`Ieee802154Engine::multipan_enable`], using this owner's MAC.
    pub fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.engine.multipan_enable(&mut self.ll)
    }

    /// [`Ieee802154Engine::multipan_receive`], using this owner's MAC.
    pub fn multipan_receive<E>(&mut self, env: &mut E, index: Ieee802154MultipanIndex)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.multipan_receive(&mut self.ll, env, index)
    }

    /// [`Ieee802154Engine::multipan_sleep`], using this owner's MAC.
    pub fn multipan_sleep<E>(&mut self, env: &mut E, index: Ieee802154MultipanIndex)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.multipan_sleep(&mut self.ll, env, index)
    }

    /// [`Ieee802154Engine::multipan_set_rx_when_idle`], using this owner's MAC.
    pub fn multipan_set_rx_when_idle(&mut self, index: Ieee802154MultipanIndex, enable: bool) {
        self.engine.multipan_set_rx_when_idle(index, enable)
    }

    /// [`Ieee802154Engine::set_panid`], using this owner's MAC.
    pub fn set_panid(&mut self, panid: u16) {
        self.engine.set_panid(&mut self.ll, panid)
    }

    /// [`Ieee802154Engine::panid`], using this owner's MAC.
    pub fn panid(&mut self) -> u16 {
        self.engine.panid(&mut self.ll)
    }

    /// [`Ieee802154Engine::set_short_address`], using this owner's MAC.
    pub fn set_short_address(&mut self, address: u16) {
        self.engine.set_short_address(&mut self.ll, address)
    }

    /// [`Ieee802154Engine::short_address`], using this owner's MAC.
    pub fn short_address(&mut self) -> u16 {
        self.engine.short_address(&mut self.ll)
    }

    /// [`Ieee802154Engine::set_extended_address`], using this owner's MAC.
    pub fn set_extended_address(&mut self, address: [u8; 8]) {
        self.engine.set_extended_address(&mut self.ll, address)
    }

    /// [`Ieee802154Engine::extended_address`], using this owner's MAC.
    pub fn extended_address(&mut self) -> [u8; 8] {
        self.engine.extended_address(&mut self.ll)
    }

    /// [`Ieee802154Engine::set_ack_timeout`], using this owner's MAC.
    pub fn set_ack_timeout(
        &mut self,
        timeout: oer_espressif_ieee802154_mac_values::Ieee802154AckTimeout,
    ) {
        self.engine.set_ack_timeout(&mut self.ll, timeout)
    }

    /// [`Ieee802154Engine::ack_timeout`], using this owner's MAC.
    pub fn ack_timeout(&mut self) -> u32 {
        self.engine.ack_timeout(&mut self.ll)
    }

    /// [`Ieee802154Engine::set_transmit_security`], using this owner's MAC.
    pub fn set_transmit_security(
        &mut self,
        frame: &[u8],
        key: &[u8; 16],
        address: &[u8; 8],
    ) -> Result<(), Ieee802154SecurityFrameError> {
        self.engine
            .set_transmit_security(&mut self.ll, frame, key, address)
    }

    /// [`Ieee802154Engine::recent_lqi`], using this owner's MAC.
    pub fn recent_lqi(&self) -> u8 {
        self.engine.recent_lqi()
    }

    /// [`Ieee802154Engine::rx_frame`], using this owner's MAC.
    pub fn rx_frame(&self, slot: Ieee802154RxSlot) -> ([u8; FRAME_SIZE], Ieee802154FrameInfo) {
        self.engine.rx_frame(slot)
    }

    /// [`Ieee802154Engine::model_dma_write`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), any(test, feature = "test-support")))]
    pub fn model_dma_write(&mut self, address: Ll::Address, image: &[u8]) -> bool {
        self.engine.model_dma_write(address, image)
    }

    /// [`Ieee802154Engine::model_rx_slot`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), any(test, feature = "test-support")))]
    pub fn model_rx_slot(&self, address: Ll::Address) -> Option<Ieee802154RxSlot> {
        self.engine.model_rx_slot(address)
    }

    /// [`Ieee802154Engine::model_transmit_address`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), any(test, feature = "test-support")))]
    pub fn model_transmit_address(&self) -> Ll::Address {
        self.engine.model_transmit_address()
    }

    /// [`Ieee802154Engine::model_dma_read`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), any(test, feature = "test-support")))]
    pub fn model_dma_read(&mut self, address: Ll::Address) -> Option<[u8; FRAME_SIZE]> {
        self.engine.model_dma_read(address)
    }

    /// [`Ieee802154Engine::enable`], using this owner's MAC.
    pub fn enable(&mut self) {
        self.engine.enable()
    }

    /// [`Ieee802154Engine::disable`], using this owner's MAC.
    pub fn disable(&mut self) {
        self.engine.disable()
    }

    /// [`Ieee802154Engine::mac_init`], using this owner's MAC.
    pub fn mac_init(&mut self) {
        self.engine.mac_init(&mut self.ll)
    }

    /// [`Ieee802154Engine::set_txrx_statistics`], using this owner's MAC.
    pub fn set_txrx_statistics(&mut self, collect: bool) {
        self.engine.set_txrx_statistics(collect)
    }

    /// [`Ieee802154Engine::txrx_statistics`], using this owner's MAC.
    pub const fn txrx_statistics(&self) -> Option<Ieee802154TxRxStatistics> {
        self.engine.txrx_statistics()
    }

    /// [`Ieee802154Engine::clear_txrx_statistics`], using this owner's MAC.
    pub fn clear_txrx_statistics(&mut self) {
        self.engine.clear_txrx_statistics()
    }

    /// [`Ieee802154Engine::mac_deinit`], using this owner's MAC.
    pub fn mac_deinit(&mut self) {
        self.engine.mac_deinit(&mut self.ll)
    }

    /// [`Ieee802154Engine::receive_handle_done`], using this owner's MAC.
    pub fn receive_handle_done(
        &mut self,
        slot: Ieee802154RxSlot,
    ) -> Result<(), Ieee802154SlotError> {
        self.engine.receive_handle_done(slot)
    }

    /// [`Ieee802154Engine::transmit`], using this owner's MAC.
    pub fn transmit<E>(
        &mut self,
        env: &mut E,
        frame: &[u8],
        cca: bool,
    ) -> Result<(), Ieee802154FrameImageError>
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.transmit(&mut self.ll, env, frame, cca)
    }

    /// [`Ieee802154Engine::transmit_at`], using this owner's MAC.
    pub fn transmit_at<E>(
        &mut self,
        env: &mut E,
        frame: &[u8],
        cca: bool,
        time: u32,
    ) -> Result<(), Ieee802154FrameImageError>
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.transmit_at(&mut self.ll, env, frame, cca, time)
    }

    /// [`Ieee802154Engine::receive`], using this owner's MAC.
    pub fn receive<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.receive(&mut self.ll, env)
    }

    /// [`Ieee802154Engine::receive_at`], using this owner's MAC.
    pub fn receive_at<E>(&mut self, env: &mut E, time: u32, duration: u32) -> bool
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.receive_at(&mut self.ll, env, time, duration)
    }

    /// [`Ieee802154Engine::sleep`], using this owner's MAC.
    pub fn sleep<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.sleep(&mut self.ll, env)
    }

    /// [`Ieee802154Engine::energy_detect`], using this owner's MAC.
    pub fn energy_detect<E>(&mut self, env: &mut E, duration: u16)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.energy_detect(&mut self.ll, env, duration)
    }

    /// [`Ieee802154Engine::cca`], using this owner's MAC.
    pub fn cca<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.engine.cca(&mut self.ll, env)
    }

    /// [`Ieee802154Engine::isr`], using this owner's MAC.
    ///
    /// # Errors
    /// Unsupported events consume this owner into absorbing quarantine.
    #[allow(
        clippy::result_large_err,
        reason = "quarantine retains the complete affine owner"
    )]
    pub fn isr<E>(mut self, env: &mut E) -> Result<Self, Quarantined<'storage, Ll>>
    where
        E: Ieee802154Environment + ?Sized,
    {
        match self.engine.isr(&mut self.ll, env) {
            Ok(()) => Ok(self),
            Err(fault) => Err(Quarantined {
                fault,
                _owners: ManuallyDrop::new(self),
            }),
        }
    }
}
