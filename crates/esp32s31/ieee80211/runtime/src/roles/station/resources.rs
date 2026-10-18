//! Role-local resources shared by every production station phase.

use core::marker::PhantomData;

use oer_esp32s31_ieee80211_mac::rx::RxDmaBufferAddresses;

use oer_ieee80211_mac::scan::ScanTable;

/// RX DMA arena and the address layout derived from that exact allocation.
///
/// The descriptor base and buffer-address table must never be recomputed from
/// a different allocation during a later station epoch. Keeping all three in
/// one owner also gives production firmware and HIL the same DMA vocabulary.
pub struct StationDmaResources<'storage, S, const COUNT: usize> {
    storage: &'storage S,
    descriptor_base: u32,
    buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
}

impl<'storage, S, const COUNT: usize> StationDmaResources<'storage, S, COUNT> {
    pub const fn new(
        storage: &'storage S,
        descriptor_base: u32,
        buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
    ) -> Self {
        Self {
            storage,
            descriptor_base,
            buffer_addresses,
        }
    }

    pub const fn storage(&self) -> &'storage S {
        self.storage
    }

    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub const fn buffer_addresses(&self) -> &'storage RxDmaBufferAddresses<COUNT> {
        self.buffer_addresses
    }

    pub fn into_parts(self) -> (&'storage S, u32, &'storage RxDmaBufferAddresses<COUNT>) {
        (self.storage, self.descriptor_base, self.buffer_addresses)
    }
}

/// Persistent physical-radio and interrupt authority owned by one station
/// service.
///
/// `O` is the complete role-neutral radio owner, not a borrow from a sibling
/// field. `I` is an exact interrupt-epoch owner or unique borrow, not a wake
/// handle. Moving both values into the finite station task avoids a
/// self-referential running state and lets the executor return the complete
/// graph to the supervisor.
pub struct StationRadioResources<'role, O, I> {
    owner: O,
    interrupt: I,
    _role: PhantomData<&'role mut ()>,
}

impl<'role, O, I> StationRadioResources<'role, O, I> {
    pub fn new(owner: O, interrupt: I) -> Self {
        Self {
            owner,
            interrupt,
            _role: PhantomData,
        }
    }

    pub fn interrupt(&self) -> &I {
        &self.interrupt
    }

    pub const fn owner(&self) -> &O {
        &self.owner
    }

    pub fn parts_mut(&mut self) -> (&mut O, &mut I) {
        (&mut self.owner, &mut self.interrupt)
    }

    pub fn into_parts(self) -> (O, I) {
        (self.owner, self.interrupt)
    }
}

/// Static station storage which moves through scan, join and connected
/// phases without exposing board allocation details to the lifecycle.
///
/// `D` groups RX DMA storage and its derived address metadata. `T` is the
/// complete reusable TX epoch. The two scratch frames deliberately remain
/// distinct: management parsing and Ethernet decapsulation may be live in the
/// same connected transition.
pub struct StationStorageResources<'storage, D, T, const RECORDS: usize> {
    dma: D,
    tx: T,
    scan_table: &'storage mut ScanTable<RECORDS>,
    management_frame: &'storage mut [u8],
    ethernet_frame: &'storage mut [u8],
}

impl<'storage, D, T, const RECORDS: usize> StationStorageResources<'storage, D, T, RECORDS> {
    pub fn new(
        dma: D,
        tx: T,
        scan_table: &'storage mut ScanTable<RECORDS>,
        management_frame: &'storage mut [u8],
        ethernet_frame: &'storage mut [u8],
    ) -> Self {
        Self {
            dma,
            tx,
            scan_table,
            management_frame,
            ethernet_frame,
        }
    }

    pub fn parts_mut(
        &mut self,
    ) -> (
        &mut D,
        &mut T,
        &mut ScanTable<RECORDS>,
        &mut [u8],
        &mut [u8],
    ) {
        (
            &mut self.dma,
            &mut self.tx,
            self.scan_table,
            self.management_frame,
            self.ethernet_frame,
        )
    }

    pub fn parts(&self) -> (&D, &T, &ScanTable<RECORDS>, &[u8], &[u8]) {
        (
            &self.dma,
            &self.tx,
            self.scan_table,
            self.management_frame,
            self.ethernet_frame,
        )
    }

    pub fn into_parts(
        self,
    ) -> (
        D,
        T,
        &'storage mut ScanTable<RECORDS>,
        &'storage mut [u8],
        &'storage mut [u8],
    ) {
        (
            self.dma,
            self.tx,
            self.scan_table,
            self.management_frame,
            self.ethernet_frame,
        )
    }
}

/// Complete reusable resource graph shared by station phases.
///
/// `B` contains composition-specific services such as spawners, observers,
/// network policy and fault hooks. It cannot contain the PAC or interrupt
/// setup token: those capabilities remain in the typed radio transition.
pub struct StationRuntimeResources<'role, 'storage, P, I, D, T, B, const RECORDS: usize> {
    radio: StationRadioResources<'role, P, I>,
    storage: StationStorageResources<'storage, D, T, RECORDS>,
    board: B,
}

impl<'role, 'storage, P, I, D, T, B, const RECORDS: usize>
    StationRuntimeResources<'role, 'storage, P, I, D, T, B, RECORDS>
{
    pub fn new(
        radio: StationRadioResources<'role, P, I>,
        storage: StationStorageResources<'storage, D, T, RECORDS>,
        board: B,
    ) -> Self {
        Self {
            radio,
            storage,
            board,
        }
    }

    pub fn split_mut(
        &mut self,
    ) -> (
        &mut StationRadioResources<'role, P, I>,
        &mut StationStorageResources<'storage, D, T, RECORDS>,
        &mut B,
    ) {
        (&mut self.radio, &mut self.storage, &mut self.board)
    }

    pub fn radio(&self) -> &StationRadioResources<'role, P, I> {
        &self.radio
    }

    pub const fn board(&self) -> &B {
        &self.board
    }

    pub fn into_parts(self) -> StationRuntimeParts<'role, 'storage, P, I, D, T, B, RECORDS> {
        StationRuntimeParts {
            radio: self.radio,
            storage: self.storage,
            board: self.board,
        }
    }
}

/// Named decomposition frontier used only after a finite station phase has
/// returned every borrow.
pub struct StationRuntimeParts<'role, 'storage, P, I, D, T, B, const RECORDS: usize> {
    pub radio: StationRadioResources<'role, P, I>,
    pub storage: StationStorageResources<'storage, D, T, RECORDS>,
    pub board: B,
}

#[cfg(test)]
mod tests;
