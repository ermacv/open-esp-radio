use oer_hil_protocol::wifi::WifiMonitorFrameChunk;

use super::*;

fn chunk(sequence: u32, offset: u16, total: u16, bytes: &[u8]) -> WifiMonitorFrameChunk {
    WifiMonitorFrameChunk::try_new(
        4, sequence, 1_000, total, total, offset, None, None, None, bytes,
    )
    .unwrap()
}

#[test]
fn ordered_chunks_reconstruct_one_packet() {
    let report = assemble(vec![chunk(0, 0, 5, &[1, 2, 3]), chunk(0, 3, 5, &[4, 5])]).unwrap();
    assert_eq!(report.incomplete_frames, 0);
    assert_eq!(report.packets.len(), 1);
    assert_eq!(report.packets[0].bytes, &[1, 2, 3, 4, 5]);
}

#[test]
fn missing_middle_chunk_discards_the_complete_frame() {
    let report = assemble(vec![chunk(0, 0, 6, &[1, 2]), chunk(0, 4, 6, &[5, 6])]).unwrap();
    assert!(report.packets.is_empty());
    assert_eq!(report.incomplete_frames, 1);
}

#[test]
fn a_new_sequence_discards_an_unfinished_predecessor() {
    let report = assemble(vec![chunk(0, 0, 4, &[1, 2]), chunk(1, 0, 2, &[3, 4])]).unwrap();
    assert_eq!(report.incomplete_frames, 1);
    assert_eq!(report.packets.len(), 1);
    assert_eq!(report.packets[0].frame_sequence, 1);
}
