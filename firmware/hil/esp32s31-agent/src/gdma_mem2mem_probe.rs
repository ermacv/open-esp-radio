//! Diagnostic PSRAM-to-SRAM AXI-GDMA promotion benchmark.
//!
//! This executes once before radio initialization. It uses the same PSRAM
//! mapping and linker profile as the Wi-Fi workload. The benchmark consumes
//! its dedicated DMA channel before radio initialization, so no DMA state
//! participates in the measured datapath.

use core::{
    arch::asm,
    future::{Future, poll_fn},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU32, Ordering},
};

use oer_esp32s31_soc_esp_hal::{
    AxiGdmaDescriptor, AxiGdmaMem2Mem, AxiGdmaMem2MemSegment, BurstSize,
};

const FRAME_SIZE: usize = 1536;
// Three full cycles of the eight scatter-gather frame lengths below: every
// length keeps its share of the batch, and the SRAM destination stays small
// enough to leave the bootstrap handoff margin intact in the diagnostic image.
const FRAMES_PER_BATCH: usize = 24;
const BATCH_SIZE: usize = FRAME_SIZE * FRAMES_PER_BATCH;
// Each source begins on an isolated cache-line boundary. The full 1,514-byte
// Ethernet geometry is proved separately below; the 24-way SG case retains a
// guard inside this existing 36-KiB benchmark allocation so the diagnostic
// does not consume more production SRAM.
const SG_FRAME_STRIDE: usize = FRAME_SIZE;
const SG_STORAGE_SIZE: usize = BATCH_SIZE;
const SG_DESTINATION_OFFSET: usize = 36;
// 4 and the production offset 36 have the same residue for every supported
// burst size. Using 4 keeps a full 1,514-byte frame inside each existing
// 1,536-byte benchmark slot without allocating another SRAM arena.
const SG_BENCHMARK_DESTINATION_OFFSET: usize = 4;
const SG_BENCHMARK_FRAME_SIZE: usize = 1514;
const SG_BENCHMARK_BYTES: usize = SG_BENCHMARK_FRAME_SIZE * FRAMES_PER_BATCH;
const ITERATIONS: usize = 64;
const CACHE_LINE: usize = 64;
const DESCRIPTOR_COUNT: usize = FRAMES_PER_BATCH;

#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct Batch([u8; SG_STORAGE_SIZE]);

#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct DescriptorBatch([AxiGdmaDescriptor; DESCRIPTOR_COUNT]);

#[repr(C, align(64))]
#[derive(bytemuck::Zeroable)]
struct CacheLine([u8; CACHE_LINE]);

oer_memory::zeroed_static! {
    static mut SOURCE: Batch =
        zeroed in ".psram.bss.gdma_mem2mem_probe.source";
}

oer_memory::zeroed_static! {
    static mut NEXT_SOURCE: Batch =
        zeroed in ".psram.bss.gdma_mem2mem_probe.next";
}

oer_memory::zeroed_static! {
    static mut DESTINATION: Batch =
        zeroed in ".dma.bss.gdma_mem2mem_probe.destination";
}

oer_memory::zeroed_static! {
    static mut SRAM_SOURCE: CacheLine =
        zeroed in ".dma.bss.gdma_mem2mem_probe.sram_source";
}

// The runtime's ordinary `.bss` deliberately lives in PSRAM. DMA descriptors
// are control structures and must remain in uncached internal SRAM even though
// AXI-GDMA can access the PSRAM payload itself.
oer_memory::zeroed_static! {
    static mut RX_DESCRIPTORS: DescriptorBatch =
        zeroed in ".dma.bss.gdma_mem2mem_probe.rx_descriptors";
}

oer_memory::zeroed_static! {
    static mut TX_DESCRIPTORS: DescriptorBatch =
        zeroed in ".dma.bss.gdma_mem2mem_probe.tx_descriptors";
}

// Kept in uncached SRAM so JTAG can recover the exact failing frontier even
// when the diagnostic runs before the asynchronous USB logger makes progress.
oer_memory::zeroed_static! {
    #[unsafe(no_mangle)]
    #[used]
    pub static OPEN_RADIO_GDMA_PROBE_STATUS: AtomicU32 =
        zeroed in ".dma.bss.gdma_mem2mem_probe.status";
}

oer_memory::zeroed_static! {
    #[unsafe(no_mangle)]
    #[used]
    pub static OPEN_RADIO_GDMA_PROBE_DETAIL: AtomicU32 =
        zeroed in ".dma.bss.gdma_mem2mem_probe.detail";
}

// Total cycles/instructions for 64 iterations of, respectively: CPU copy,
// blocking GDMA, CPU copy + next-batch preparation, serial GDMA + preparation,
// overlapped GDMA + preparation, interrupt-driven GDMA, realistic 24-frame
// scatter copy, realistic 24-frame interrupt-driven scatter GDMA wall time,
// and the exact task-active subset of that asynchronous transfer.
oer_memory::zeroed_static! {
    #[unsafe(no_mangle)]
    #[used]
    pub static OPEN_RADIO_GDMA_PROBE_CYCLES: [AtomicU32; 9] =
        zeroed in ".dma.bss.gdma_mem2mem_probe.cycles";
}

oer_memory::zeroed_static! {
    #[unsafe(no_mangle)]
    #[used]
    pub static OPEN_RADIO_GDMA_PROBE_INSTRUCTIONS: [AtomicU32; 9] =
        zeroed in ".dma.bss.gdma_mem2mem_probe.instructions";
}

fn mark(status: u32, detail: u32) {
    OPEN_RADIO_GDMA_PROBE_DETAIL.store(detail, Ordering::Relaxed);
    OPEN_RADIO_GDMA_PROBE_STATUS.store(status, Ordering::Release);
}

#[inline(always)]
fn cycles() -> u32 {
    let value: u32;
    unsafe { asm!("rdcycle {value}", value = out(reg) value, options(nomem, nostack)) };
    value
}

#[inline(always)]
fn instructions() -> u32 {
    let value: u32;
    unsafe { asm!("rdinstret {value}", value = out(reg) value, options(nomem, nostack)) };
    value
}

#[inline(never)]
fn dirty_batch(batch: &mut [u8], salt: u8) {
    for (line, bytes) in batch.chunks_exact_mut(CACHE_LINE).enumerate() {
        bytes[0] = salt.wrapping_add(line as u8);
    }
}

#[inline(never)]
fn prepare_next_batch(batch: &mut [u8], salt: u8) -> u32 {
    let mut checksum = 0u32;
    for (index, byte) in batch.iter_mut().enumerate() {
        *byte = (index as u8)
            .wrapping_mul(29)
            .wrapping_add(salt.rotate_left((index & 7) as u32));
        checksum = checksum.rotate_left(5) ^ u32::from(*byte);
    }
    core::hint::black_box(checksum)
}

#[inline(never)]
fn fill_pattern(batch: &mut [u8], salt: u8) {
    for (index, byte) in batch.iter_mut().enumerate() {
        *byte = (index as u8)
            .wrapping_mul(37)
            .wrapping_add(salt.rotate_left((index & 7) as u32));
    }
}

#[inline(never)]
fn fill_volatile(batch: &mut [u8], value: u8) {
    for byte in batch {
        unsafe { ptr::write_volatile(byte, value) };
    }
}

fn first_mismatch(left: &[u8], right: &[u8]) -> Option<(usize, u8, u8)> {
    left.iter()
        .zip(right)
        .enumerate()
        .find_map(|(index, (&left, &right))| (left != right).then_some((index, left, right)))
}

const fn sg_frame_length(index: usize) -> usize {
    match index % 8 {
        0 => 64,
        1 => 73,
        2 => 511,
        3 => 1020,
        4 => 1492,
        5 => 1500,
        6 => 257,
        _ => 1400,
    }
}

const fn sg_batch_bytes() -> usize {
    let mut frame = 0;
    let mut bytes = 0;
    while frame < FRAMES_PER_BATCH {
        bytes += sg_frame_length(frame);
        frame += 1;
    }
    bytes
}

const SG_BATCH_BYTES: usize = sg_batch_bytes();

fn prepare_sg_storage(source: &mut [u8], destination: &mut [u8], salt: u8) {
    destination.fill(0xa5);
    for frame in 0..FRAMES_PER_BATCH {
        let start = frame * SG_FRAME_STRIDE;
        let length = sg_frame_length(frame);
        fill_pattern(
            &mut source[start..start + length],
            salt.wrapping_add(frame as u8),
        );
        source[start + length..start + SG_FRAME_STRIDE].fill(0x3c);
    }
}

fn prepare_sg_benchmark_storage(source: &mut [u8], destination: &mut [u8], salt: u8) {
    destination.fill(0xa5);
    for frame in 0..FRAMES_PER_BATCH {
        let start = frame * SG_FRAME_STRIDE;
        fill_pattern(
            &mut source[start..start + SG_BENCHMARK_FRAME_SIZE],
            salt.wrapping_add(frame as u8),
        );
        source[start + SG_BENCHMARK_FRAME_SIZE..start + SG_FRAME_STRIDE].fill(0x3c);
    }
}

#[inline(never)]
fn copy_sg_benchmark_frames(source: &[u8], destination: &mut [u8]) {
    for frame in 0..FRAMES_PER_BATCH {
        let start = frame * SG_FRAME_STRIDE;
        destination[start + SG_BENCHMARK_DESTINATION_OFFSET
            ..start + SG_BENCHMARK_DESTINATION_OFFSET + SG_BENCHMARK_FRAME_SIZE]
            .copy_from_slice(&source[start..start + SG_BENCHMARK_FRAME_SIZE]);
    }
}

fn sg_segments<'a>(
    source: &'a mut [u8; SG_STORAGE_SIZE],
    destination: &'a mut [u8; SG_STORAGE_SIZE],
) -> [AxiGdmaMem2MemSegment<'a>; FRAMES_PER_BATCH] {
    let source_frames: &mut [[u8; SG_FRAME_STRIDE]; FRAMES_PER_BATCH] = source
        .as_mut_slice()
        .as_chunks_mut::<SG_FRAME_STRIDE>()
        .0
        .try_into()
        .unwrap();
    let destination_frames: &mut [[u8; SG_FRAME_STRIDE]; FRAMES_PER_BATCH] = destination
        .as_mut_slice()
        .as_chunks_mut::<SG_FRAME_STRIDE>()
        .0
        .try_into()
        .unwrap();
    let mut source_frames = source_frames.each_mut().map(Some);
    let mut destination_frames = destination_frames.each_mut().map(Some);
    core::array::from_fn(|index| {
        let length = sg_frame_length(index);
        let source = source_frames[index]
            .take()
            .expect("one unique source frame per SG segment");
        let destination = destination_frames[index]
            .take()
            .expect("one unique destination frame per SG segment");
        AxiGdmaMem2MemSegment::new(
            &mut destination[SG_DESTINATION_OFFSET..SG_DESTINATION_OFFSET + length],
            &mut source[..length],
        )
    })
}

fn sg_benchmark_segments<'a>(
    source: &'a mut [u8; SG_STORAGE_SIZE],
    destination: &'a mut [u8; SG_STORAGE_SIZE],
) -> [AxiGdmaMem2MemSegment<'a>; FRAMES_PER_BATCH] {
    let source_frames: &mut [[u8; SG_FRAME_STRIDE]; FRAMES_PER_BATCH] = source
        .as_mut_slice()
        .as_chunks_mut::<SG_FRAME_STRIDE>()
        .0
        .try_into()
        .unwrap();
    let destination_frames: &mut [[u8; SG_FRAME_STRIDE]; FRAMES_PER_BATCH] = destination
        .as_mut_slice()
        .as_chunks_mut::<SG_FRAME_STRIDE>()
        .0
        .try_into()
        .unwrap();
    let mut source_frames = source_frames.each_mut().map(Some);
    let mut destination_frames = destination_frames.each_mut().map(Some);
    core::array::from_fn(|index| {
        let source = source_frames[index]
            .take()
            .expect("one unique source frame per SG benchmark segment");
        let destination = destination_frames[index]
            .take()
            .expect("one unique destination frame per SG benchmark segment");
        AxiGdmaMem2MemSegment::new(
            &mut destination[SG_BENCHMARK_DESTINATION_OFFSET
                ..SG_BENCHMARK_DESTINATION_OFFSET + SG_BENCHMARK_FRAME_SIZE],
            &mut source[..SG_BENCHMARK_FRAME_SIZE],
        )
    })
}

fn verify_sg_segments(
    segments: &[AxiGdmaMem2MemSegment<'_>],
    active: usize,
) -> Result<(), (usize, usize, u8, u8)> {
    for (frame, segment) in segments.iter().take(active).enumerate() {
        if let Some((offset, actual, expected)) =
            first_mismatch(segment.destination(), segment.source())
        {
            return Err((frame, offset, actual, expected));
        }
    }
    Ok(())
}

fn verify_sg_guards(destination: &[u8], active: usize) -> Result<(), (usize, usize, u8)> {
    for frame in 0..active {
        let frame_start = frame * SG_FRAME_STRIDE;
        let length = sg_frame_length(frame);
        let prefix = &destination[frame_start..frame_start + SG_DESTINATION_OFFSET];
        if let Some((offset, &actual)) = prefix.iter().enumerate().find(|(_, byte)| **byte != 0xa5)
        {
            return Err((frame, offset, actual));
        }
        let payload_end = frame_start + SG_DESTINATION_OFFSET + length;
        let frame_end = frame_start + SG_FRAME_STRIDE;
        if let Some((offset, &actual)) = destination[payload_end..frame_end]
            .iter()
            .enumerate()
            .find(|(_, byte)| **byte != 0xa5)
        {
            return Err((frame, SG_DESTINATION_OFFSET + length + offset, actual));
        }
    }
    Ok(())
}

fn report(slot: usize, label: &str, elapsed_cycles: u32, retired_instructions: u32) {
    report_bytes(
        slot,
        label,
        elapsed_cycles,
        retired_instructions,
        BATCH_SIZE,
    );
}

fn report_bytes(
    slot: usize,
    label: &str,
    elapsed_cycles: u32,
    retired_instructions: u32,
    bytes_per_batch: usize,
) {
    OPEN_RADIO_GDMA_PROBE_CYCLES[slot].store(elapsed_cycles, Ordering::Release);
    OPEN_RADIO_GDMA_PROBE_INSTRUCTIONS[slot].store(retired_instructions, Ordering::Release);
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem mode={} cycles={} instructions={} cycles_batch={} cycles_frame={} bytes={}",
        label,
        elapsed_cycles,
        retired_instructions,
        elapsed_cycles / ITERATIONS as u32,
        elapsed_cycles / (ITERATIONS * FRAMES_PER_BATCH) as u32,
        bytes_per_batch * ITERATIONS,
    );
}

// The sole consumed channel grants this invocation exclusive access to the
// dedicated static probe buffers/descriptors. Every started transfer is either
// completed or dropped before another iteration reuses those allocations.
// Cancelling this future drops its transfer first; forgetting the whole future
// retains the channel and static storage indefinitely without another caller.
pub(crate) async fn run(channel: oer_esp32s31_soc_esp_hal::AxiGdmaMem2MemChannel<'static>) {
    mark(0x0001, 0);
    let source = unsafe { &mut *ptr::addr_of_mut!(SOURCE) };
    let next_source = unsafe { &mut *ptr::addr_of_mut!(NEXT_SOURCE) };
    let destination = unsafe { &mut *ptr::addr_of_mut!(DESTINATION) };
    let sram_source = unsafe { &mut *ptr::addr_of_mut!(SRAM_SOURCE) };
    let rx_descriptors = unsafe { &mut *ptr::addr_of_mut!(RX_DESCRIPTORS) };
    let tx_descriptors = unsafe { &mut *ptr::addr_of_mut!(TX_DESCRIPTORS) };

    assert!((0x5000_0000..0x5400_0000).contains(&(source.0.as_ptr() as usize)));
    assert!((0x5000_0000..0x5400_0000).contains(&(next_source.0.as_ptr() as usize)));
    assert!((0x2f00_0000..0x2f08_0000).contains(&(destination.0.as_ptr() as usize)));

    for (index, byte) in source.0.iter_mut().enumerate() {
        *byte = index as u8;
    }
    for (index, byte) in next_source.0.iter_mut().enumerate() {
        *byte = !(index as u8);
    }

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        destination.0[..BATCH_SIZE].copy_from_slice(&source.0[..BATCH_SIZE]);
        core::hint::black_box(&destination.0[..BATCH_SIZE]);
    }
    report(
        0,
        "cpu-copy-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let mut gdma = AxiGdmaMem2Mem::new(channel);
    mark(0x0010, 0);

    fill_pattern(&mut sram_source.0, 0x71);
    fill_volatile(&mut destination.0[..CACHE_LINE], 0xa5);
    mark(0x0011, CACHE_LINE as u32);
    let transfer = match gdma.prepare(
        &mut destination.0[..CACHE_LINE],
        &mut sram_source.0,
        &mut rx_descriptors.0,
        &mut tx_descriptors.0,
        BurstSize::Bytes32,
    ) {
        Ok(transfer) => transfer,
        Err(error) => {
            mark(0xe010, CACHE_LINE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=prepare error={:?}",
                error,
            );
            return;
        }
    };
    // SAFETY: this run retains the exclusive static probe storage until cleanup.
    let sram_report = match unsafe { transfer.start() }.wait_blocking(100_000_000) {
        Ok(report) => report,
        Err(error) => {
            mark(0xe020, CACHE_LINE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=wait error={:?}",
                error,
            );
            return;
        }
    };
    if let Some((offset, actual, expected)) =
        first_mismatch(&destination.0[..CACHE_LINE], &sram_source.0)
    {
        let detail =
            ((offset as u32 & 0xffff) << 16) | (u32::from(actual) << 8) | u32::from(expected);
        mark(0xe030, detail);
        log::error!(
            "OPEN_RADIO_HIL gdma_mem2mem sram_control=FAIL stage=compare offset={} actual={} expected={}",
            offset,
            actual,
            expected,
        );
        return;
    }
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem sram_control=PASS status={:?}",
        sram_report.status,
    );

    for (case, size) in [64usize, 1536, 4032, 4096, BATCH_SIZE]
        .into_iter()
        .enumerate()
    {
        fill_pattern(&mut source.0[..size], 0x31u8.wrapping_add(case as u8));
        fill_volatile(&mut destination.0[..size], 0xa5);
        mark(0x0100 + case as u32, size as u32);
        let transfer = match gdma.prepare(
            &mut destination.0[..size],
            &mut source.0[..size],
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        ) {
            Ok(transfer) => transfer,
            Err(error) => {
                mark(0xe100 + case as u32, size as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=prepare size={} error={:?}",
                    size,
                    error,
                );
                return;
            }
        };
        mark(0x0200 + case as u32, size as u32);
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        let report = match unsafe { transfer.start() }.wait_blocking(100_000_000) {
            Ok(report) => report,
            Err(error) => {
                mark(0xe200 + case as u32, size as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=wait size={} error={:?}",
                    size,
                    error,
                );
                return;
            }
        };
        mark(0x0300 + case as u32, size as u32);
        if let Some((offset, actual, expected)) =
            first_mismatch(&destination.0[..size], &source.0[..size])
        {
            let detail =
                ((offset as u32 & 0xffff) << 16) | (u32::from(actual) << 8) | u32::from(expected);
            mark(0xe300 + case as u32, detail);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem correctness=FAIL stage=compare size={} offset={} actual={} expected={}",
                size,
                offset,
                actual,
                expected,
            );
            return;
        }
        log::info!(
            "OPEN_RADIO_HIL gdma_mem2mem correctness=PASS size={} descriptors={} status={:?}",
            size,
            report.descriptors,
            report.status,
        );
    }

    const ETHERNET_FRAME_SIZE: usize = 1514;
    const ETHERNET_GUARD_END: usize = 1600;
    fill_pattern(&mut source.0[..ETHERNET_FRAME_SIZE], 0x6d);
    fill_volatile(&mut destination.0[..ETHERNET_GUARD_END], 0xa5);
    mark(0x0380, ETHERNET_FRAME_SIZE as u32);
    let transfer = match gdma.prepare(
        &mut destination.0[SG_DESTINATION_OFFSET..SG_DESTINATION_OFFSET + ETHERNET_FRAME_SIZE],
        &mut source.0[..ETHERNET_FRAME_SIZE],
        &mut rx_descriptors.0,
        &mut tx_descriptors.0,
        BurstSize::Bytes32,
    ) {
        Ok(transfer) => transfer,
        Err(error) => {
            mark(0xe380, ETHERNET_FRAME_SIZE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem ethernet_geometry=FAIL stage=prepare error={:?}",
                error,
            );
            return;
        }
    };
    // SAFETY: this run retains the exclusive static probe storage until cleanup.
    let ethernet_report = match unsafe { transfer.start() }.wait_blocking(100_000_000) {
        Ok(report) => report,
        Err(error) => {
            mark(0xe381, ETHERNET_FRAME_SIZE as u32);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem ethernet_geometry=FAIL stage=wait error={:?}",
                error,
            );
            return;
        }
    };
    if let Some((offset, actual, expected)) = first_mismatch(
        &destination.0[SG_DESTINATION_OFFSET..SG_DESTINATION_OFFSET + ETHERNET_FRAME_SIZE],
        &source.0[..ETHERNET_FRAME_SIZE],
    ) {
        let detail =
            ((offset as u32 & 0xffff) << 16) | (u32::from(actual) << 8) | u32::from(expected);
        mark(0xe382, detail);
        log::error!(
            "OPEN_RADIO_HIL gdma_mem2mem ethernet_geometry=FAIL stage=compare offset={} actual={} expected={}",
            offset,
            actual,
            expected,
        );
        return;
    }
    if destination.0[..SG_DESTINATION_OFFSET]
        .iter()
        .chain(
            destination.0[SG_DESTINATION_OFFSET + ETHERNET_FRAME_SIZE..ETHERNET_GUARD_END].iter(),
        )
        .any(|byte| *byte != 0xa5)
    {
        mark(0xe383, ETHERNET_FRAME_SIZE as u32);
        log::error!("OPEN_RADIO_HIL gdma_mem2mem ethernet_geometry=FAIL stage=guard");
        return;
    }
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem ethernet_geometry=PASS offset={} bytes={} descriptors={} status={:?}",
        SG_DESTINATION_OFFSET,
        ethernet_report.bytes,
        ethernet_report.descriptors,
        ethernet_report.status,
    );

    // Prove the actual Wi-Fi promotion geometry rather than extrapolating
    // from one contiguous benchmark allocation. Sources occupy independent
    // cache-line-isolated PSRAM slots; destinations use the production
    // 36-byte Ethernet offset in independent internal-SRAM slots. Mixed frame
    // lengths deliberately include non-burst-aligned values.
    for (case, active) in [1usize, 2].into_iter().enumerate() {
        prepare_sg_storage(&mut source.0, &mut destination.0, 0x80 + case as u8);
        let report = {
            let mut segments = sg_segments(&mut source.0, &mut destination.0);
            mark(0x0400 + case as u32, active as u32);
            let transfer = match gdma.prepare_segments(
                &mut segments[..active],
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            ) {
                Ok(transfer) => transfer,
                Err(error) => {
                    mark(0xe400 + case as u32, active as u32);
                    log::error!(
                        "OPEN_RADIO_HIL gdma_mem2mem scatter_gather=FAIL stage=prepare frames={} error={:?}",
                        active,
                        error,
                    );
                    return;
                }
            };
            // SAFETY: this run retains the exclusive static probe storage until cleanup.
            let report = match unsafe { transfer.start() }.wait_blocking(100_000_000) {
                Ok(report) => report,
                Err(error) => {
                    mark(0xe410 + case as u32, active as u32);
                    log::error!(
                        "OPEN_RADIO_HIL gdma_mem2mem scatter_gather=FAIL stage=wait frames={} error={:?}",
                        active,
                        error,
                    );
                    return;
                }
            };
            if let Err((frame, offset, actual, expected)) = verify_sg_segments(&segments, active) {
                let detail = ((frame as u32 & 0xff) << 24)
                    | ((offset as u32 & 0xfff) << 12)
                    | (u32::from(actual) << 8)
                    | u32::from(expected);
                mark(0xe420 + case as u32, detail);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem scatter_gather=FAIL stage=compare frames={} frame={} offset={} actual={} expected={}",
                    active,
                    frame,
                    offset,
                    actual,
                    expected,
                );
                return;
            }
            report
        };
        if let Err((frame, offset, actual)) = verify_sg_guards(&destination.0, active) {
            let detail =
                ((frame as u32 & 0xff) << 24) | ((offset as u32 & 0xffff) << 8) | u32::from(actual);
            mark(0xe430 + case as u32, detail);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem scatter_gather=FAIL stage=guard frames={} frame={} offset={} actual={}",
                active,
                frame,
                offset,
                actual,
            );
            return;
        }
        log::info!(
            "OPEN_RADIO_HIL gdma_mem2mem scatter_gather=PASS frames={} bytes={} descriptors={} status={:?}",
            active,
            report.bytes,
            report.descriptors,
            report.status,
        );
    }

    // Dropping an active owner must synchronously stop/reset the channel and
    // preserve every byte outside the declared destinations. The following
    // full asynchronous batch is also the recovery proof for that reset.
    prepare_sg_storage(&mut source.0, &mut destination.0, 0xa0);
    {
        let mut segments = sg_segments(&mut source.0, &mut destination.0);
        let transfer = match gdma.prepare_segments(
            &mut segments,
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        ) {
            // SAFETY: this run retains the exclusive static probe storage until cleanup.
            Ok(transfer) => unsafe { transfer.start() },
            Err(error) => {
                mark(0xe440, FRAMES_PER_BATCH as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem cancellation=FAIL stage=prepare error={:?}",
                    error,
                );
                return;
            }
        };
        drop(transfer);
    }
    if let Err((frame, offset, actual)) = verify_sg_guards(&destination.0, FRAMES_PER_BATCH) {
        let detail =
            ((frame as u32 & 0xff) << 24) | ((offset as u32 & 0xffff) << 8) | u32::from(actual);
        mark(0xe450, detail);
        log::error!(
            "OPEN_RADIO_HIL gdma_mem2mem cancellation=FAIL stage=guard frame={} offset={} actual={}",
            frame,
            offset,
            actual,
        );
        return;
    }
    log::info!("OPEN_RADIO_HIL gdma_mem2mem cancellation=PASS");

    prepare_sg_storage(&mut source.0, &mut destination.0, 0xc0);
    let sg_report = {
        let mut segments = sg_segments(&mut source.0, &mut destination.0);
        mark(0x0500, FRAMES_PER_BATCH as u32);
        let report = match gdma.prepare_segments(
            &mut segments,
            &mut rx_descriptors.0,
            &mut tx_descriptors.0,
            BurstSize::Bytes32,
        ) {
            // SAFETY: this run retains the exclusive static probe storage until cleanup.
            Ok(transfer) => match unsafe { transfer.start() }.await {
                Ok(report) => report,
                Err(error) => {
                    mark(0xe510, FRAMES_PER_BATCH as u32);
                    log::error!(
                        "OPEN_RADIO_HIL gdma_mem2mem scatter_gather_async=FAIL stage=wait error={:?}",
                        error,
                    );
                    return;
                }
            },
            Err(error) => {
                mark(0xe500, FRAMES_PER_BATCH as u32);
                log::error!(
                    "OPEN_RADIO_HIL gdma_mem2mem scatter_gather_async=FAIL stage=prepare error={:?}",
                    error,
                );
                return;
            }
        };
        if let Err((frame, offset, actual, expected)) =
            verify_sg_segments(&segments, FRAMES_PER_BATCH)
        {
            let detail = ((frame as u32 & 0xff) << 24)
                | ((offset as u32 & 0xfff) << 12)
                | (u32::from(actual) << 8)
                | u32::from(expected);
            mark(0xe520, detail);
            log::error!(
                "OPEN_RADIO_HIL gdma_mem2mem scatter_gather_async=FAIL stage=compare frame={} offset={} actual={} expected={}",
                frame,
                offset,
                actual,
                expected,
            );
            return;
        }
        report
    };
    if let Err((frame, offset, actual)) = verify_sg_guards(&destination.0, FRAMES_PER_BATCH) {
        let detail =
            ((frame as u32 & 0xff) << 24) | ((offset as u32 & 0xffff) << 8) | u32::from(actual);
        mark(0xe530, detail);
        log::error!(
            "OPEN_RADIO_HIL gdma_mem2mem scatter_gather_async=FAIL stage=guard frame={} offset={} actual={}",
            frame,
            offset,
            actual,
        );
        return;
    }
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem scatter_gather_async=PASS frames={} bytes={} descriptors={} status={:?}",
        FRAMES_PER_BATCH,
        sg_report.bytes,
        sg_report.descriptors,
        sg_report.status,
    );

    let mut elapsed_cycles = 0u32;
    let mut retired_instructions = 0u32;
    let mut active_cycles = 0u32;
    let mut active_instructions = 0u32;
    for iteration in 0..ITERATIONS {
        prepare_sg_benchmark_storage(
            &mut source.0,
            &mut destination.0,
            0xd0_u8.wrapping_add(iteration as u8),
        );
        let started_cycles = cycles();
        let started_instructions = instructions();
        copy_sg_benchmark_frames(&source.0, &mut destination.0);
        elapsed_cycles = elapsed_cycles.wrapping_add(cycles().wrapping_sub(started_cycles));
        retired_instructions =
            retired_instructions.wrapping_add(instructions().wrapping_sub(started_instructions));
        core::hint::black_box(destination.0[SG_DESTINATION_OFFSET]);
    }
    report_bytes(
        6,
        "cpu-scatter-dirty",
        elapsed_cycles,
        retired_instructions,
        SG_BENCHMARK_BYTES,
    );

    let mut elapsed_cycles = 0u32;
    let mut retired_instructions = 0u32;
    for iteration in 0..ITERATIONS {
        prepare_sg_benchmark_storage(
            &mut source.0,
            &mut destination.0,
            0xe0_u8.wrapping_add(iteration as u8),
        );
        let started_cycles = cycles();
        let started_instructions = instructions();
        let mut segments = sg_benchmark_segments(&mut source.0, &mut destination.0);
        let prepared = gdma
            .prepare_segments(
                &mut segments,
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap();
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        let mut transfer = unsafe { prepared.start() };
        active_cycles = active_cycles.wrapping_add(cycles().wrapping_sub(started_cycles));
        active_instructions =
            active_instructions.wrapping_add(instructions().wrapping_sub(started_instructions));
        poll_fn(|context| {
            let poll_started_cycles = cycles();
            let poll_started_instructions = instructions();
            let result = Pin::new(&mut transfer).poll(context);
            active_cycles = active_cycles.wrapping_add(cycles().wrapping_sub(poll_started_cycles));
            active_instructions = active_instructions
                .wrapping_add(instructions().wrapping_sub(poll_started_instructions));
            result
        })
        .await
        .unwrap();
        drop(transfer);
        elapsed_cycles = elapsed_cycles.wrapping_add(cycles().wrapping_sub(started_cycles));
        retired_instructions =
            retired_instructions.wrapping_add(instructions().wrapping_sub(started_instructions));
        core::hint::black_box(segments[0].destination()[0]);
    }
    report_bytes(
        7,
        "gdma-scatter-async-dirty",
        elapsed_cycles,
        retired_instructions,
        SG_BENCHMARK_BYTES,
    );
    report_bytes(
        8,
        "gdma-scatter-async-active",
        active_cycles,
        active_instructions,
        SG_BENCHMARK_BYTES,
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        let prepared = gdma
            .prepare(
                &mut destination.0[..BATCH_SIZE],
                &mut source.0[..BATCH_SIZE],
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap();
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        unsafe { prepared.start() }
            .wait_blocking(100_000_000)
            .unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        1,
        "gdma-blocking-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let mut checksum = 0u32;
    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        destination.0[..BATCH_SIZE].copy_from_slice(&source.0[..BATCH_SIZE]);
        checksum ^= prepare_next_batch(&mut next_source.0[..BATCH_SIZE], iteration as u8);
        core::hint::black_box(&destination.0[..BATCH_SIZE]);
    }
    report(
        2,
        "cpu-copy-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        let prepared = gdma
            .prepare(
                &mut destination.0[..BATCH_SIZE],
                &mut source.0[..BATCH_SIZE],
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap();
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        unsafe { prepared.start() }
            .wait_blocking(100_000_000)
            .unwrap();
        checksum ^= prepare_next_batch(&mut next_source.0[..BATCH_SIZE], iteration as u8);
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        3,
        "gdma-serial-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        let prepared = gdma
            .prepare(
                &mut destination.0[..BATCH_SIZE],
                &mut source.0[..BATCH_SIZE],
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap();
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        let transfer = unsafe { prepared.start() };
        checksum ^= prepare_next_batch(&mut next_source.0[..BATCH_SIZE], iteration as u8);
        transfer.wait_blocking(100_000_000).unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        4,
        "gdma-overlap-next-prep",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    let started_cycles = cycles();
    let started_instructions = instructions();
    for iteration in 0..ITERATIONS {
        dirty_batch(&mut source.0[..BATCH_SIZE], iteration as u8);
        let prepared = gdma
            .prepare(
                &mut destination.0[..BATCH_SIZE],
                &mut source.0[..BATCH_SIZE],
                &mut rx_descriptors.0,
                &mut tx_descriptors.0,
                BurstSize::Bytes32,
            )
            .unwrap();
        // SAFETY: this run retains the exclusive static probe storage until cleanup.
        unsafe { prepared.start() }.await.unwrap();
        core::hint::black_box(destination.0[iteration % BATCH_SIZE]);
    }
    report(
        5,
        "gdma-async-dirty",
        cycles().wrapping_sub(started_cycles),
        instructions().wrapping_sub(started_instructions),
    );

    assert_eq!(&destination.0[..BATCH_SIZE], &source.0[..BATCH_SIZE]);
    mark(0x600d, BATCH_SIZE as u32);
    log::info!(
        "OPEN_RADIO_HIL gdma_mem2mem result=PASS checksum={} ethernet_offset={} ethernet_bytes={} scatter_cases=1,2,32 scatter_bytes={} cancellation=true",
        checksum,
        SG_DESTINATION_OFFSET,
        ETHERNET_FRAME_SIZE,
        SG_BATCH_BYTES,
    );
}
