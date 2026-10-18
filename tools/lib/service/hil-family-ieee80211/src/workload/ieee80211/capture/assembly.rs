use oer_hil_protocol::{
    wifi::WifiMonitorFrameChunk, wifi::WifiMonitorObserved, wifi::WifiMonitorPhyEvidence,
};

use crate::Result;

#[derive(Debug)]
pub(super) struct CapturedPacket {
    pub generation: u32,
    pub frame_sequence: u32,
    pub dequeued_at_micros: u64,
    pub logical_length: u32,
    pub channel: Option<WifiMonitorObserved<u8>>,
    pub rssi_dbm: Option<WifiMonitorObserved<i8>>,
    pub rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
    pub bytes: Vec<u8>,
}

pub(super) struct AssemblyReport {
    pub packets: Vec<CapturedPacket>,
    pub incomplete_frames: u32,
}

struct PendingFrame {
    packet: CapturedPacket,
    captured_length: usize,
}

pub(super) fn assemble(chunks: Vec<WifiMonitorFrameChunk>) -> Result<AssemblyReport> {
    let mut packets = Vec::new();
    let mut pending: Option<PendingFrame> = None;
    let mut incomplete_frames = 0_u32;

    for chunk in chunks {
        if pending
            .as_ref()
            .is_some_and(|frame| frame.packet.frame_sequence != chunk.frame_sequence)
        {
            incomplete_frames = incomplete_frames.saturating_add(1);
            pending = None;
        }

        if pending.is_none() {
            if chunk.offset != 0 {
                incomplete_frames = incomplete_frames.saturating_add(1);
                continue;
            }
            pending = Some(PendingFrame {
                packet: CapturedPacket {
                    generation: chunk.generation,
                    frame_sequence: chunk.frame_sequence,
                    dequeued_at_micros: chunk.dequeued_at_micros,
                    logical_length: u32::from(chunk.logical_length),
                    channel: chunk.channel,
                    rssi_dbm: chunk.rssi_dbm,
                    rate: chunk.rate,
                    bytes: Vec::with_capacity(usize::from(chunk.captured_length)),
                },
                captured_length: usize::from(chunk.captured_length),
            });
        }

        let frame = pending.as_mut().expect("pending frame was initialized");
        let metadata_matches = frame.packet.generation == chunk.generation
            && frame.packet.dequeued_at_micros == chunk.dequeued_at_micros
            && frame.packet.logical_length == u32::from(chunk.logical_length)
            && frame.packet.channel == chunk.channel
            && frame.packet.rssi_dbm == chunk.rssi_dbm
            && frame.packet.rate == chunk.rate
            && frame.captured_length == usize::from(chunk.captured_length);
        if !metadata_matches || usize::from(chunk.offset) != frame.packet.bytes.len() {
            incomplete_frames = incomplete_frames.saturating_add(1);
            pending = None;
            continue;
        }
        frame.packet.bytes.extend_from_slice(chunk.bytes());
        if frame.packet.bytes.len() > frame.captured_length {
            return Err("monitor chunk assembly exceeded its declared captured length".into());
        }
        if chunk.is_final() {
            let completed = pending.take().expect("final chunk has one pending frame");
            if completed.packet.bytes.len() != completed.captured_length {
                incomplete_frames = incomplete_frames.saturating_add(1);
            } else {
                packets.push(completed.packet);
            }
        }
    }

    if pending.is_some() {
        incomplete_frames = incomplete_frames.saturating_add(1);
    }
    Ok(AssemblyReport {
        packets,
        incomplete_frames,
    })
}

#[cfg(test)]
mod tests;
