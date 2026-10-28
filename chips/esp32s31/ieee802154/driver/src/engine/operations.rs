//! Mutable semantic entries; no mutable borrow of the family owner escapes.
use super::*;
use oer_espressif_ieee802154_engine::{
    coex::Ieee802154Coexistence,
    engine::{
        Ieee802154Environment, Ieee802154FrameImageError, Ieee802154RxSlot,
        Ieee802154SecurityFrameError, Ieee802154SlotError, PENDING_TABLE_SIZE,
    },
    pib::Ieee802154Pib,
};
use oer_espressif_ieee802154_mac_values::{Ieee802154MultipanEnableState, Ieee802154MultipanIndex};
use oer_ieee802154_port::{AutoPendingMode, PendingTable};

impl<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>> Ieee802154Engine<Ll> {
    /// The recording MAC, for a host model to latch events and inspect publications.
    #[cfg(all(not(target_arch = "riscv32"), test))]
    pub fn model_ll(&mut self) -> &mut Ll {
        self.inner.model_ll()
    }

    /// [`SharedEngine::set_coexistence`], using this owner's MAC.
    pub fn set_coexistence(&mut self, coexistence: Ieee802154Coexistence) {
        self.inner.set_coexistence(coexistence)
    }

    /// [`SharedEngine::update_pib`], applying RX changes in this entry.
    pub fn update_pib<E: Ieee802154Environment + ?Sized>(
        &mut self,
        env: &mut E,
        update: impl FnOnce(&mut Ieee802154Pib),
    ) {
        self.inner.update_pib(env, update)
    }

    /// [`SharedEngine::set_pending_mode`], applying at the safe RX point.
    pub fn set_pending_mode<E: Ieee802154Environment + ?Sized>(
        &mut self,
        env: &mut E,
        index: Ieee802154MultipanIndex,
        mode: AutoPendingMode,
    ) {
        self.inner.set_pending_mode(env, index, mode)
    }

    /// [`SharedEngine::pending_table`], using this owner's MAC.
    pub fn pending_table(&mut self) -> &mut PendingTable<PENDING_TABLE_SIZE> {
        self.inner.pending_table()
    }

    /// [`SharedEngine::pending_table_for`], using this owner's MAC.
    pub fn pending_table_for(
        &mut self,
        index: Ieee802154MultipanIndex,
    ) -> &mut PendingTable<PENDING_TABLE_SIZE> {
        self.inner.pending_table_for(index)
    }

    /// [`SharedEngine::set_multipan_panid`], using this owner's MAC.
    pub fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.inner.set_multipan_panid(index, panid)
    }

    /// [`SharedEngine::multipan_panid`], using this owner's MAC.
    pub fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.inner.multipan_panid(index)
    }

    /// [`SharedEngine::set_multipan_short_address`], using this owner's MAC.
    pub fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.inner.set_multipan_short_address(index, address)
    }

    /// [`SharedEngine::multipan_short_address`], using this owner's MAC.
    pub fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.inner.multipan_short_address(index)
    }

    /// [`SharedEngine::set_multipan_extended_address`], using this owner's MAC.
    pub fn set_multipan_extended_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: [u8; 8],
    ) {
        self.inner.set_multipan_extended_address(index, address)
    }

    /// [`SharedEngine::multipan_extended_address`], using this owner's MAC.
    pub fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.inner.multipan_extended_address(index)
    }

    /// [`SharedEngine::set_multipan_enable`], using this owner's MAC.
    pub fn set_multipan_enable(&mut self, enable: Ieee802154MultipanEnableState) {
        self.inner.set_multipan_enable(enable)
    }

    /// [`SharedEngine::multipan_enable`], using this owner's MAC.
    pub fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.inner.multipan_enable()
    }

    /// [`SharedEngine::multipan_receive`], using this owner's MAC.
    pub fn multipan_receive<E>(&mut self, env: &mut E, index: Ieee802154MultipanIndex)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.multipan_receive(env, index)
    }

    /// [`SharedEngine::multipan_sleep`], using this owner's MAC.
    pub fn multipan_sleep<E>(&mut self, env: &mut E, index: Ieee802154MultipanIndex)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.multipan_sleep(env, index)
    }

    /// [`SharedEngine::multipan_set_rx_when_idle`], using this owner's MAC.
    pub fn multipan_set_rx_when_idle(&mut self, index: Ieee802154MultipanIndex, enable: bool) {
        self.inner.multipan_set_rx_when_idle(index, enable)
    }

    /// [`SharedEngine::set_panid`], using this owner's MAC.
    pub fn set_panid(&mut self, panid: u16) {
        self.inner.set_panid(panid)
    }

    /// [`SharedEngine::panid`], using this owner's MAC.
    pub fn panid(&mut self) -> u16 {
        self.inner.panid()
    }

    /// [`SharedEngine::set_short_address`], using this owner's MAC.
    pub fn set_short_address(&mut self, address: u16) {
        self.inner.set_short_address(address)
    }

    /// [`SharedEngine::short_address`], using this owner's MAC.
    pub fn short_address(&mut self) -> u16 {
        self.inner.short_address()
    }

    /// [`SharedEngine::set_extended_address`], using this owner's MAC.
    pub fn set_extended_address(&mut self, address: [u8; 8]) {
        self.inner.set_extended_address(address)
    }

    /// [`SharedEngine::extended_address`], using this owner's MAC.
    pub fn extended_address(&mut self) -> [u8; 8] {
        self.inner.extended_address()
    }

    /// [`SharedEngine::set_ack_timeout`], using this owner's MAC.
    pub fn set_ack_timeout(
        &mut self,
        timeout: oer_espressif_ieee802154_mac_values::Ieee802154AckTimeout,
    ) {
        self.inner.set_ack_timeout(timeout)
    }

    /// [`SharedEngine::ack_timeout`], using this owner's MAC.
    pub fn ack_timeout(&mut self) -> u32 {
        self.inner.ack_timeout()
    }

    /// [`SharedEngine::set_transmit_security`], using this owner's MAC.
    pub fn set_transmit_security(
        &mut self,
        frame: &[u8],
        key: &[u8; 16],
        address: &[u8; 8],
    ) -> Result<(), Ieee802154SecurityFrameError> {
        self.inner.set_transmit_security(frame, key, address)
    }

    /// [`SharedEngine::model_dma_write`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), test))]
    pub fn model_dma_write(&mut self, address: DmaWindowAddress, image: &[u8]) -> bool {
        self.inner.model_dma_write(address, image)
    }

    /// [`SharedEngine::model_dma_read`], using this owner's MAC.
    #[cfg(all(not(target_arch = "riscv32"), test))]
    pub fn model_dma_read(
        &mut self,
        address: DmaWindowAddress,
    ) -> Option<[u8; oer_espressif_ieee802154_mac_values::FRAME_SIZE]> {
        self.inner.model_dma_read(address)
    }

    /// [`SharedEngine::enable`], using this owner's MAC.
    pub fn enable(&mut self) {
        self.inner.enable()
    }

    /// [`SharedEngine::disable`], using this owner's MAC.
    pub fn disable(&mut self) {
        self.inner.disable()
    }

    /// [`SharedEngine::mac_init`], using this owner's MAC.
    pub fn mac_init(&mut self) {
        self.inner.mac_init()
    }

    /// [`SharedEngine::set_txrx_statistics`], using this owner's MAC.
    pub fn set_txrx_statistics(&mut self, collect: bool) {
        self.inner.set_txrx_statistics(collect)
    }

    /// [`SharedEngine::clear_txrx_statistics`], using this owner's MAC.
    pub fn clear_txrx_statistics(&mut self) {
        self.inner.clear_txrx_statistics()
    }

    /// [`SharedEngine::mac_deinit`], using this owner's MAC.
    pub fn mac_deinit(&mut self) {
        self.inner.mac_deinit()
    }

    /// [`SharedEngine::receive_handle_done`], using this owner's MAC.
    pub fn receive_handle_done(
        &mut self,
        slot: Ieee802154RxSlot,
    ) -> Result<(), Ieee802154SlotError> {
        self.inner.receive_handle_done(slot)
    }

    /// [`SharedEngine::transmit`], using this owner's MAC.
    pub fn transmit<E>(
        &mut self,
        env: &mut E,
        frame: &[u8],
        cca: bool,
    ) -> Result<(), Ieee802154FrameImageError>
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.transmit(env, frame, cca)
    }

    /// [`SharedEngine::transmit_at`], using this owner's MAC.
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
        self.inner.transmit_at(env, frame, cca, time)
    }

    /// [`SharedEngine::receive`], using this owner's MAC.
    pub fn receive<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.receive(env)
    }

    /// [`SharedEngine::receive_at`], using this owner's MAC.
    pub fn receive_at<E>(&mut self, env: &mut E, time: u32, duration: u32) -> bool
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.receive_at(env, time, duration)
    }

    /// [`SharedEngine::sleep`], using this owner's MAC.
    pub fn sleep<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.sleep(env)
    }

    /// [`SharedEngine::energy_detect`], using this owner's MAC.
    pub fn energy_detect<E>(&mut self, env: &mut E, duration: u16)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.energy_detect(env, duration)
    }

    /// [`SharedEngine::cca`], using this owner's MAC.
    pub fn cca<E>(&mut self, env: &mut E)
    where
        E: Ieee802154Environment + ?Sized,
    {
        self.inner.cca(env)
    }
}
