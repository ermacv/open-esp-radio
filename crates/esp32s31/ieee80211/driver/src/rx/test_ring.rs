//! Host fixtures of the product receive service: a live ring over a leaked
//! host DMA arena, completed as the DMA engine would, an S31 RX-control
//! prefix builder, and [`CountingSink`], the publisher that counts and
//! releases each lease at once.

use std::{boxed::Box, vec, vec::Vec};

use oer_esp32s31_ieee80211_dma::{
    descriptor::{LENGTH_SHIFT, OWNER, RX_DONE},
    rx_dma::model::RxWalkerModel,
    rx_storage::RxDmaStorage,
    space::HostDma,
};
use oer_esp32s31_ieee80211_mac::rx::{
    PUBLIC_HEADER_SIZE, RxRingLive,
    pool::{NetworkRxFrame, RxStagePool},
};

use super::{
    counters::WifiRxCounters,
    sink::{CountingAdmission, frame_control_preview, unclassified_preview},
    transaction::{CompletedUnit, Counters, Preview, Publisher, service},
};
use crate::datapath::DatapathRxProgress;

pub(crate) const COUNT: usize = 16;
pub(crate) const BUFFER: usize = 1_664;
pub(crate) const SLOTS: usize = 4;

type Storage = RxDmaStorage<COUNT, BUFFER, { BUFFER + 4 }, HostDma>;

pub(crate) struct Ring {
    storage: &'static Storage,
    buffers: &'static [u32; COUNT],
    base: u32,
    walker: RxWalkerModel,
    pub(crate) live: RxRingLive<'static, COUNT>,
    pub(crate) pool: &'static RxStagePool<SLOTS, BUFFER>,
    next: usize,
}

impl Ring {
    pub(crate) fn new() -> Self {
        let storage: &'static Storage = Box::leak(Box::new(Storage::new()));
        let buffers = Box::leak(Box::new([0; COUNT]));
        let base = storage.dma_layout(buffers).unwrap();
        let buffers: &'static [u32; COUNT] = buffers;
        let mut walker = RxWalkerModel::new();
        let live = storage
            .prepare_ring(&mut walker, base, buffers)
            .unwrap()
            .try_start(&mut walker)
            .map_err(|(_, error)| error)
            .unwrap();
        Self {
            storage,
            buffers,
            base,
            walker,
            live,
            pool: Box::leak(Box::new(RxStagePool::new())),
            next: 0,
        }
    }

    /// Complete one unit of `lengths` segments as the DMA engine would:
    /// every non-terminal buffer is written, the terminal carries `RX_DONE`.
    pub(crate) fn complete(&mut self, lengths: &[u32]) {
        for (step, &length) in lengths.iter().enumerate() {
            let index = self.next + step;
            let terminal = step + 1 == lengths.len();
            if !terminal {
                self.storage
                    .dma_write(&self.live, self.buffers[index], &[0; 4])
                    .unwrap();
            }
            self.storage.descriptors()[index].write_word0(
                BUFFER as u32
                    | (length << LENGTH_SHIFT)
                    | if terminal { RX_DONE } else { 0 }
                    | OWNER,
            );
        }
        self.next += lengths.len();
        self.walker
            .set_cursor(self.base, self.next - 1, Some(self.next));
    }

    /// Complete one single-descriptor unit whose received bytes are `bytes`.
    pub(crate) fn complete_with(&mut self, bytes: &[u8]) {
        self.storage
            .dma_write(&self.live, self.buffers[self.next], bytes)
            .unwrap();
        self.complete(&[bytes.len() as u32]);
    }

    /// Run the product service until it settles, as the runner does: on a
    /// drained frontier or a pass blocked on staging capacity.
    pub(crate) fn settle<E>(
        &mut self,
        publisher: &E,
        admission: &CountingAdmission<'_>,
    ) -> DatapathRxProgress
    where
        E: Publisher<'static, BUFFER, SLOTS>,
    {
        let (mut descriptors, mut units, mut bytes) = (0, 0, 0);
        for _ in 0..4 {
            let progress = service(
                &mut self.live,
                self.storage,
                self.pool,
                publisher,
                admission,
                Counters {
                    descriptors: &mut descriptors,
                    units: &mut units,
                    bytes: &mut bytes,
                },
                &mut self.walker,
                (),
            )
            .expect("received input never faults the ring");
            if matches!(
                progress,
                DatapathRxProgress::Drained | DatapathRxProgress::StageCapacityBlocked
            ) {
                return progress;
            }
        }
        panic!("the service settles within four passes");
    }

    /// Run the product service until the frontier drained.
    pub(crate) fn drain<E>(&mut self, publisher: &E, admission: &CountingAdmission<'_>)
    where
        E: Publisher<'static, BUFFER, SLOTS>,
    {
        assert_eq!(
            self.settle(publisher, admission),
            DatapathRxProgress::Drained
        );
    }
}

/// An S31 RX-control prefix: the fixed 0x38 bytes and the dynamic tail
/// before the MPDU at 0x40.
#[derive(Clone, Copy)]
pub(crate) struct S31Prefix {
    pub(crate) rssi: i8,
    /// The raw MAC local time at 0x0c.
    pub(crate) stamp: u32,
    /// The RXEND state at 0x08.
    pub(crate) rxend_state: u8,
    /// The tail's RX state: `0xf5` MIC failure, `0xc6` quarantined.
    pub(crate) rx_state: u8,
    /// `sig_len` instead of the MPDU length plus the FCS.
    pub(crate) signal_length: Option<u16>,
    /// The dump length instead of `sig_len` plus the FCS.
    pub(crate) dump_length: Option<u16>,
}

impl S31Prefix {
    /// A legacy OFDM prefix of a cleanly received frame.
    pub(crate) const fn new() -> Self {
        Self {
            rssi: -50,
            stamp: 0x1000_0000,
            rxend_state: 0,
            rx_state: 0,
            signal_length: None,
            dump_length: None,
        }
    }

    /// The received bytes of a unit carrying `mpdu` behind this prefix.
    pub(crate) fn unit(&self, mpdu: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; PUBLIC_HEADER_SIZE + mpdu.len()];
        bytes[0] = self.rssi as u8;
        bytes[8] = self.rxend_state;
        bytes[0x0c..0x10].copy_from_slice(&self.stamp.to_le_bytes());
        bytes[0x25] = 1 << 4;
        let signal_length = self
            .signal_length
            .unwrap_or((mpdu.len() + 4).try_into().unwrap());
        let dump_length = self.dump_length.unwrap_or(signal_length + 4);
        bytes[0x38..0x3a].copy_from_slice(&signal_length.to_le_bytes());
        bytes[0x3a..0x3c].copy_from_slice(&dump_length.to_le_bytes());
        bytes[0x3c] = self.rx_state;
        bytes[PUBLIC_HEADER_SIZE..].copy_from_slice(mpdu);
        bytes
    }
}

/// A 24-byte management header of `subtype` (8 beacon, 5 probe response).
pub(crate) fn management_header(subtype: u8) -> [u8; 24] {
    let mut header = [0; 24];
    header[0] = subtype << 4;
    header[4..10].fill(0xff);
    header[10..16].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x01]);
    header[16..22].copy_from_slice(&[0x02, 0, 0, 0, 0, 0x01]);
    header
}

/// A publisher that counts each unit and releases its lease at once.
pub(crate) struct CountingSink<'a> {
    counters: &'a WifiRxCounters,
}

impl<'a> CountingSink<'a> {
    pub(crate) const fn new(counters: &'a WifiRxCounters) -> Self {
        Self { counters }
    }
}

impl<'pool, const CAPACITY: usize, const STAGE_SLOTS: usize> Publisher<'pool, CAPACITY, STAGE_SLOTS>
    for CountingSink<'_>
{
    const DEPTH: usize = STAGE_SLOTS;

    /// Every lease is released inside `try_send`, so the sink never fills.
    fn free_capacity(&self) -> usize {
        STAGE_SLOTS
    }

    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        frame_control_preview(unit, bytes)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        unclassified_preview(unit)
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, STAGE_SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, STAGE_SLOTS, CAPACITY>> {
        self.counters.received(frame.length());
        drop(frame);
        Ok(())
    }
}
