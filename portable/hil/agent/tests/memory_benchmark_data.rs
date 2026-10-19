//! Exercise the target's actual conditioning and validation on host memory.

use oer_hil_agent::memory_benchmark as data;

use oer_hil_protocol::{
    system::MemoryBenchmarkMode, system::MemoryBenchmarkRequest, system::MemoryBenchmarkSource,
    system::MemoryBenchmarkStop,
};

const LENGTHS: &[usize] = &[1, 31, 32, 33, 63, 64, 65, 1514, 4095, 4096];
const OFFSET: usize = data::OFFSET;

#[test]
fn completed_copies_validate_terminal_lengths_and_cache_line_edges() {
    for &length in LENGTHS {
        let (layout, source, mut destination) = batch(length as u16, 1, 0);
        layout.copy(&source, &mut destination);
        assert_eq!(layout.verify(&source, &destination), Ok(()));
    }
}

#[test]
fn payload_corruption_is_detected_at_both_edges_and_interior() {
    for &length in LENGTHS {
        let (layout, source, mut destination) = batch(length as u16, 1, 7);
        layout.copy(&source, &mut destination);
        for index in [0, length / 2, length - 1] {
            destination[OFFSET + index] ^= 1;
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::DataMismatch),
                "length={length}, payload index={index}",
            );
            destination[OFFSET + index] ^= 1;
        }
    }
}

#[test]
fn duplicated_source_blocks_cannot_qualify_as_a_successful_copy() {
    for iteration in [0, 3, 4, 67] {
        let (layout, source, mut destination) = batch(4096, 1, iteration);
        for block_start in (256..layout.bytes).step_by(256) {
            layout.copy(&source, &mut destination);
            // Model a transfer that repeats its first block at a later address.
            destination[OFFSET + block_start..OFFSET + block_start + 256]
                .copy_from_slice(&source[..256]);
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::DataMismatch),
                "duplicated block accepted at offset {block_start}, iteration {iteration}",
            );
        }
    }
}

#[test]
fn underflow_and_overflow_are_detected_throughout_both_guards() {
    for &length in LENGTHS {
        let (layout, source, mut destination) = batch(length as u16, 1, 11);
        layout.copy(&source, &mut destination);
        for index in (0..OFFSET).chain(OFFSET + length..layout.stride) {
            destination[index] ^= 1;
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::GuardCorrupted),
                "length={length}, guard index={index}",
            );
            destination[index] ^= 1;
        }
    }
}

#[test]
fn every_iteration_rejects_stale_payload_and_reconditions_the_destination() {
    let (layout, mut source, mut destination) = batch(4096, 1, 0);
    let mut previous = source[..layout.bytes].to_vec();
    for iteration in 0..64 {
        // A previous transfer may have damaged payload or either guard.
        destination.fill(0xff);
        layout.prepare(&mut source, &mut destination, iteration);
        if iteration > 0 {
            assert!(
                source[..layout.bytes]
                    .iter()
                    .zip(&previous)
                    .all(|(new, old)| new != old)
            );
            destination[payload(layout, 0)].copy_from_slice(&previous);
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::DataMismatch),
                "stale payload accepted at iteration {iteration}",
            );
        }
        layout.copy(&source, &mut destination);
        assert_eq!(layout.verify(&source, &destination), Ok(()));
        previous.copy_from_slice(&source[..layout.bytes]);
    }
}

fn request(bytes: u16, frames: u8) -> MemoryBenchmarkRequest {
    MemoryBenchmarkRequest {
        mode: MemoryBenchmarkMode::CpuCopy,
        source: MemoryBenchmarkSource::Sram,
        bytes,
        frames,
        iterations: 1,
    }
}

fn batch(bytes: u16, frames: u8, iteration: u16) -> (data::Layout, Vec<u8>, Vec<u8>) {
    let layout = data::Layout::new(request(bytes, frames)).unwrap();
    let mut source = vec![0; data::ARENA_CAPACITY];
    let mut destination = vec![0; data::ARENA_CAPACITY];
    layout.prepare(&mut source, &mut destination, iteration);
    (layout, source, destination)
}

fn payload(layout: data::Layout, frame: usize) -> std::ops::Range<usize> {
    let start = frame * layout.stride + data::OFFSET;
    start..start + layout.bytes
}

#[test]
fn every_admitted_batch_fits_the_arena_with_disjoint_cache_lines_and_guards() {
    // Exhaust the protocol's byte/frame domain and include rejected edges.
    for frames in 0..=33 {
        for bytes in 0..=4097 {
            let request = request(bytes, frames);
            let layout = data::Layout::new(request);
            assert_eq!(layout.is_some(), request.validate(), "{request:?}");
            let Some(layout) = layout else { continue };
            assert_eq!(layout.bytes, usize::from(bytes));
            assert_eq!(layout.frames, usize::from(frames));
            assert!(layout.frames <= data::MAX_FRAMES);
            assert!(layout.frames * layout.stride <= data::ARENA_CAPACITY);
            // The arenas themselves are aligned by the target owner. Each
            // source starts on its own line and owns its final rounded line.
            assert!(layout.stride.is_multiple_of(64));
            assert!(layout.bytes.next_multiple_of(64) <= layout.stride);
            assert!(data::OFFSET + layout.bytes < layout.stride);
        }
    }
}

#[test]
fn cpu_batch_copy_preserves_every_frame_at_each_frame_counts_payload_limit() {
    for frames in 1..=32 {
        let largest = (1..=4096)
            .rev()
            .find(|&bytes| request(bytes, frames).validate())
            .unwrap();
        for bytes in [1, 27, 28, 63, 64, 65, 1514, largest] {
            let (layout, source, mut destination) = batch(bytes, frames, 5);
            layout.copy(&source, &mut destination);
            for frame in 0..layout.frames {
                let start = frame * layout.stride;
                assert_eq!(
                    &destination[payload(layout, frame)],
                    &source[start..start + layout.bytes],
                    "bytes={bytes}, frames={frames}, frame={frame}",
                );
            }
            assert_eq!(layout.verify(&source, &destination), Ok(()));
        }
    }
}

#[test]
fn duplicated_and_swapped_frames_are_rejected_even_for_single_byte_frames() {
    for frames in [2, 3, 32] {
        for bytes in [1, 32, 257, 1514] {
            let (layout, source, mut destination) = batch(bytes, frames, 9);
            for frame in 1..layout.frames {
                layout.copy(&source, &mut destination);
                destination[payload(layout, frame)].copy_from_slice(&source[..layout.bytes]);
                assert_eq!(
                    layout.verify(&source, &destination),
                    Err(MemoryBenchmarkStop::DataMismatch),
                    "duplicated frame {frame}, bytes={bytes}, frames={frames}",
                );
            }
            layout.copy(&source, &mut destination);
            let last = layout.frames - 1;
            let start = last * layout.stride;
            destination[payload(layout, 0)].copy_from_slice(&source[start..start + layout.bytes]);
            destination[payload(layout, last)].copy_from_slice(&source[..layout.bytes]);
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::DataMismatch),
                "swapped first/last frames, bytes={bytes}, frames={frames}",
            );
        }
    }
}

#[test]
fn skipped_last_or_multiple_frames_are_detected_throughout_warmup_and_measurement() {
    for iteration in 0..68 {
        for (bytes, frames) in [(1, 1), (1, 3), (1, 32), (1514, 32), (4096, 12)] {
            let (layout, source, mut destination) = batch(bytes, frames, iteration);
            let untouched = destination.clone();
            for skipped in [
                vec![layout.frames - 1],
                (0..layout.frames).step_by(2).collect(),
            ] {
                layout.copy(&source, &mut destination);
                for frame in skipped {
                    let range = payload(layout, frame);
                    destination[range.clone()].copy_from_slice(&untouched[range]);
                }
                assert_eq!(
                    layout.verify(&source, &destination),
                    Err(MemoryBenchmarkStop::DataMismatch),
                    "skipped frame accepted: bytes={bytes}, frames={frames}, iteration={iteration}",
                );
            }
        }
    }
}

#[test]
fn inter_frame_guards_and_unused_arena_are_checked_at_maximum_batch_sizes() {
    for (bytes, frames) in [(4096, 1), (1, 32), (1536, 32), (4096, 12)] {
        let (layout, source, mut destination) = batch(bytes, frames, 4);
        layout.copy(&source, &mut destination);
        let mut guards = Vec::new();
        for frame in 0..layout.frames {
            let start = frame * layout.stride;
            let range = payload(layout, frame);
            guards.extend([start, range.start - 1, range.end, start + layout.stride - 1]);
        }
        let inactive = layout.frames * layout.stride;
        if inactive < destination.len() {
            guards.extend([inactive, destination.len() - 1]);
        }
        for index in guards {
            destination[index] ^= 1;
            assert_eq!(
                layout.verify(&source, &destination),
                Err(MemoryBenchmarkStop::GuardCorrupted),
                "bytes={bytes}, frames={frames}, guard index={index}",
            );
            destination[index] ^= 1;
        }
    }
}
