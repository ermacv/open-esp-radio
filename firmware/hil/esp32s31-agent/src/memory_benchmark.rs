//! Commanded pre-radio copy measurements; this task owns one DMA channel.
//!
//! Sources are CPU-written before each measured iteration. Preparation and
//! verification are outside the interval; GDMA cache writeback is inside it.
//! A failed case quarantines these static allocations until the next reset.

mod counters;
use oer_hil_agent::memory_benchmark as data;

use data::{ARENA_CAPACITY, Layout, MAX_FRAMES, OFFSET};

use core::{
    future::{Future, poll_fn},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU8, Ordering},
};
use counters::{Counters, memory_fence};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, with_timeout};
use oer_esp32s31_soc_esp_hal::{
    AxiGdmaDescriptor, AxiGdmaMem2Mem, AxiGdmaMem2MemSegment, AxiGdmaMem2MemTransferError,
    BurstSize,
};
use oer_hil_protocol::{
    base::RejectReason, system::MemoryBenchmarkEvidence, system::MemoryBenchmarkMode,
    system::MemoryBenchmarkRequest, system::MemoryBenchmarkSource, system::MemoryBenchmarkStop,
};

const WARMUPS: u16 = 4;
const BLOCKING_POLL_LIMIT: u32 = 100_000;
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(100);

#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct Source([u8; ARENA_CAPACITY]);
#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct Destination([u8; ARENA_CAPACITY]);
#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct Descriptors([AxiGdmaDescriptor; MAX_FRAMES * 2]);

oer_memory::zeroed_static! {
    static mut PSRAM_SOURCE: Source =
        zeroed in ".psram.bss.memory_benchmark.source";
}
oer_memory::zeroed_static! {
    static mut SRAM_SOURCE: Source =
        zeroed in ".dma.bss.memory_benchmark.source";
}
oer_memory::zeroed_static! {
    static mut DESTINATION: Destination =
        zeroed in ".dma.bss.memory_benchmark.destination";
}
oer_memory::zeroed_static! {
    static mut RX: Descriptors =
        zeroed in ".dma.bss.memory_benchmark.rx";
}
oer_memory::zeroed_static! {
    static mut TX: Descriptors =
        zeroed in ".dma.bss.memory_benchmark.tx";
}

struct Request {
    id: u32,
    configuration: MemoryBenchmarkRequest,
}
static REQUESTS: Channel<CriticalSectionRawMutex, Request, 1> = Channel::new();
// 0 = available, 1 = request in flight, 2 = quarantined until reset.
static STATE: AtomicU8 = AtomicU8::new(0);

pub(crate) fn submit(id: u32, configuration: MemoryBenchmarkRequest) -> Result<(), RejectReason> {
    match STATE.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(2) => return Err(RejectReason::InvalidState),
        Err(_) => return Err(RejectReason::Busy),
    }
    if REQUESTS.try_send(Request { id, configuration }).is_err() {
        STATE.store(0, Ordering::Release);
        return Err(RejectReason::Busy);
    }
    Ok(())
}

#[embassy_executor::task]
pub(crate) async fn task(channel: oer_esp32s31_soc_esp_hal::AxiGdmaMem2MemChannel<'static>) {
    // SAFETY: the uniquely consumed peripheral admits this task only once.
    // This task retains all static allocations independently of any borrowed
    // transfer. No CPU accesses them during DMA. On any failure the mailbox
    // is quarantined; forgetting the task retains both channel and storage.
    let (psram, sram, destination, rx, tx) = unsafe {
        (
            &mut *ptr::addr_of_mut!(PSRAM_SOURCE),
            &mut *ptr::addr_of_mut!(SRAM_SOURCE),
            &mut *ptr::addr_of_mut!(DESTINATION),
            &mut *ptr::addr_of_mut!(RX),
            &mut *ptr::addr_of_mut!(TX),
        )
    };
    let mut gdma = AxiGdmaMem2Mem::new(channel);
    loop {
        let command = REQUESTS.receive().await;
        let source = match command.configuration.source {
            MemoryBenchmarkSource::Sram => &mut sram.0,
            MemoryBenchmarkSource::Psram => &mut psram.0,
        };
        let report = run_case(
            &mut gdma,
            command.configuration,
            source,
            &mut destination.0,
            &mut rx.0,
            &mut tx.0,
        )
        .await;
        let next = if report.stop == MemoryBenchmarkStop::Completed {
            0
        } else {
            2
        };
        STATE.store(next, Ordering::Release);
        crate::console::publish_event_reliably(
            0,
            command.id,
            oer_hil_protocol::system::MemoryBenchmarkCompleted(report),
        )
        .await;
    }
}

async fn run_case(
    gdma: &mut AxiGdmaMem2Mem<'_>,
    request: MemoryBenchmarkRequest,
    source: &mut [u8],
    destination: &mut [u8],
    rx: &mut [AxiGdmaDescriptor],
    tx: &mut [AxiGdmaDescriptor],
) -> MemoryBenchmarkEvidence {
    let mut report = MemoryBenchmarkEvidence {
        request,
        completed_iterations: 0,
        elapsed_micros: 0,
        elapsed_cycles: 0,
        elapsed_instructions: 0,
        foreground_cycles: 0,
        foreground_instructions: 0,
        polls: 0,
        stop: MemoryBenchmarkStop::Completed,
    };
    let layout = Layout::new(request).expect("admitted benchmark fits its static arena");
    for iteration in 0..request.iterations + WARMUPS {
        layout.prepare(source, destination, iteration);
        memory_fence();
        let started = Instant::now();
        let counters = Counters::read();
        let mut foreground = Counters::default();
        let mut polls = 0;
        let result = match request.mode {
            MemoryBenchmarkMode::CpuCopy => {
                layout.copy(source, destination);
                memory_fence();
                Ok(())
            }
            MemoryBenchmarkMode::GdmaBlocking | MemoryBenchmarkMode::GdmaAsync => {
                // Segment assembly is part of the measured preparation cost.
                // chunks_mut establishes disjoint leases without raw pointers.
                let mut segments: heapless::Vec<_, MAX_FRAMES> = source
                    .chunks_mut(layout.stride)
                    .zip(destination.chunks_mut(layout.stride))
                    .take(layout.frames)
                    .map(|(source, destination)| {
                        AxiGdmaMem2MemSegment::new(
                            &mut destination[OFFSET..OFFSET + layout.bytes],
                            &mut source[..layout.bytes],
                        )
                    })
                    .collect();
                match gdma.prepare_segments(&mut segments, rx, tx, BurstSize::Bytes32) {
                    Err(_) => {
                        foreground.add(Counters::read().since(counters));
                        Err(MemoryBenchmarkStop::PrepareFailed)
                    }
                    Ok(prepared) => {
                        // SAFETY: the task retains exclusively claimed static
                        // storage through completion/cleanup, even on cancellation.
                        let mut transfer = unsafe { prepared.start() };
                        if request.mode == MemoryBenchmarkMode::GdmaBlocking {
                            let result = transfer.wait_blocking(BLOCKING_POLL_LIMIT);
                            memory_fence();
                            result.map(|_| ()).map_err(transfer_error)
                        } else {
                            foreground.add(Counters::read().since(counters));
                            let result = with_timeout(
                                TRANSFER_TIMEOUT,
                                poll_fn(|context| {
                                    polls += 1;
                                    let started = Counters::read();
                                    let result = Pin::new(&mut transfer).poll(context);
                                    foreground.add(Counters::read().since(started));
                                    result
                                }),
                            )
                            .await;
                            let cleanup = Counters::read();
                            drop(transfer);
                            memory_fence();
                            foreground.add(Counters::read().since(cleanup));
                            match result {
                                Ok(result) => result.map(|_| ()).map_err(transfer_error),
                                Err(_) => Err(MemoryBenchmarkStop::TimedOut),
                            }
                        }
                    }
                }
            }
        };
        let elapsed = Counters::read().since(counters);
        let elapsed_micros = started.elapsed().as_micros();
        if request.mode != MemoryBenchmarkMode::GdmaAsync {
            foreground = elapsed;
        }
        if iteration >= WARMUPS {
            report.elapsed_micros += elapsed_micros;
            report.elapsed_cycles += elapsed.cycles;
            report.elapsed_instructions += elapsed.instructions;
            report.foreground_cycles += foreground.cycles;
            report.foreground_instructions += foreground.instructions;
            report.polls += polls;
        }
        if let Err(stop) = result {
            report.stop = stop;
            break;
        }
        // All bytes and both guards are observed outside the timing boundary.
        // No iteration qualifies merely because a DMA descriptor completed.
        if let Err(stop) = layout.verify(source, destination) {
            report.stop = stop;
            break;
        }
        if iteration >= WARMUPS {
            report.completed_iterations += 1;
        }
        embassy_futures::yield_now().await;
    }
    report
}

fn transfer_error(error: AxiGdmaMem2MemTransferError) -> MemoryBenchmarkStop {
    match error {
        AxiGdmaMem2MemTransferError::Timeout => MemoryBenchmarkStop::TimedOut,
        _ => MemoryBenchmarkStop::TransferFailed,
    }
}
