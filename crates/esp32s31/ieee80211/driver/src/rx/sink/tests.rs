use std::boxed::Box;

use oer_esp32s31_ieee80211_dma::{
    descriptor::{BIT_30, BIT_31, LENGTH_SHIFT, SIZE_MASK},
    rx_dma::model::RxWalkerModel,
    rx_storage::RxDmaStorage,
    space::HostDma,
};
use oer_esp32s31_ieee80211_mac::rx::{RxRingLive, pool::RxStagePool};

use super::*;
use crate::{
    datapath::DatapathRxProgress,
    rx::transaction::{Counters, Discard, Unavailable, service},
};

const COUNT: usize = 16;
const BUFFER: usize = 1_664;
const SLOTS: usize = 4;

type Storage = RxDmaStorage<COUNT, BUFFER, { BUFFER + 4 }, HostDma>;

struct Ring {
    storage: &'static Storage,
    buffers: &'static [u32; COUNT],
    base: u32,
    walker: RxWalkerModel,
    live: RxRingLive<'static, COUNT>,
    pool: &'static RxStagePool<SLOTS, BUFFER>,
    next: usize,
}

impl Ring {
    fn new() -> Self {
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
    /// every non-terminal buffer is written, the terminal carries bit 30.
    fn complete(&mut self, lengths: &[u32]) {
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
                    | if terminal { BIT_30 } else { 0 }
                    | BIT_31,
            );
        }
        self.next += lengths.len();
        self.walker
            .set_cursor(self.base, self.next - 1, Some(self.next));
    }

    /// Run the product service until it drained, as the runner does.
    fn drain(&mut self, sink: &CountingSink<'_>, admission: &CountingAdmission<'_>) {
        let (mut descriptors, mut units, mut bytes) = (0, 0, 0);
        for _ in 0..4 {
            let progress = service(
                &mut self.live,
                self.storage,
                self.pool,
                sink,
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
            if progress == DatapathRxProgress::Drained {
                return;
            }
        }
        panic!("the frontier drains within four passes");
    }
}

#[test]
fn chained_frame_is_a_counted_drop_and_the_next_frame_is_received() {
    let counters: &'static WifiRxCounters = Box::leak(Box::new(WifiRxCounters::new()));
    let (sink, admission) = (
        CountingSink::new(counters),
        CountingAdmission::new(counters),
    );
    let mut ring = Ring::new();

    ring.complete(&[BUFFER as u32, 100]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!((report.chained_drops, report.units), (1, 0));

    ring.complete(&[300]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!(
        (report.units, report.bytes, report.chained_drops),
        (1, 300, 1)
    );

    // A chain longer than the length field is counted apart.
    let mut oversized = [BUFFER as u32; 10];
    oversized[9] = 1_500;
    assert!(oversized.iter().sum::<u32>() > SIZE_MASK);
    ring.complete(&oversized);
    ring.complete(&[400]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!(
        (
            report.units,
            report.bytes,
            report.chained_drops,
            report.oversized_drops
        ),
        (2, 700, 1, 1)
    );
    assert!(ring.live.topology_snapshot().valid);
}

#[test]
fn the_sink_classifies_by_frame_control_and_never_fills() {
    let counters = WifiRxCounters::new();
    let sink = CountingSink::new(&counters);
    let unit = CompletedUnit {
        head_index: 0,
        descriptor_count: 1,
        payload_length: 64,
    };
    let mut header = [0; 24];
    header[..2].copy_from_slice(&0x4088_u16.to_le_bytes());
    let preview = Publisher::<'_, 64, 2>::preview(&sink, unit, header);
    assert_eq!(preview.frame_control, Some(0x4088));
    assert_eq!(preview.class, IngressClass::BulkProtectedData);
    assert_eq!(preview.route, IngressRoute::Standalone);
    assert_eq!(
        Publisher::<'_, 64, 2>::unclassified_preview(&sink, unit).class,
        IngressClass::Unclassified
    );
    assert_eq!(Publisher::<'_, 64, 2>::free_capacity(&sink), 2);
    // Admission keeps the default reserve and overload policy.
    let admission = CountingAdmission::new(&counters);
    assert_eq!(
        admission
            .credit_policy()
            .validate(2, 2)
            .unwrap()
            .critical_reserved_credits(),
        1
    );
    assert_eq!(
        admission.unavailable_disposition(preview),
        Unavailable::DiscardAndRecycle
    );
}

#[test]
fn admission_counts_every_drop_by_its_reason() {
    let counters = WifiRxCounters::new();
    let admission = CountingAdmission::new(&counters);
    let unit = CompletedUnit {
        head_index: 0,
        descriptor_count: 1,
        payload_length: 0,
    };
    let preview = Preview {
        unit,
        frame_control: None,
        class: IngressClass::Unclassified,
        route: IngressRoute::Malformed,
    };
    for reason in [
        Discard::Empty,
        Discard::TooLong,
        Discard::Chained,
        Discard::Oversized,
        Discard::OverloadBulk,
    ] {
        admission.observe(Observation::DiscardRetained { unit, reason });
    }
    admission.observe(Observation::OverloadDiscardedAndRecycled(preview));
    admission.observe(Observation::BulkAdmissionBlocked(preview));
    admission.observe(Observation::CriticalAdmissionBlocked(preview));
    admission.observe(Observation::Staged(unit));
    admission.observe(Observation::CriticalReserveAdmitted(preview));
    let report = counters.report();
    assert_eq!(
        (
            report.empty_drops,
            report.too_long_drops,
            report.chained_drops,
            report.oversized_drops,
            report.overload_drops,
            report.admission_blocks,
            report.units,
        ),
        (1, 1, 1, 1, 2, 2, 0)
    );
}
