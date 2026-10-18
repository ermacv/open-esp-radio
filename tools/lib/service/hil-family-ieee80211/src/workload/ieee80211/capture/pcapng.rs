use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

use crate::workload::ieee80211::capture::assembly::CapturedPacket;

const SECTION_HEADER_BLOCK: u32 = 0x0a0d_0d0a;
const INTERFACE_DESCRIPTION_BLOCK: u32 = 0x0000_0001;
const ENHANCED_PACKET_BLOCK: u32 = 0x0000_0006;
const BYTE_ORDER_MAGIC: u32 = 0x1a2b_3c4d;
const LINKTYPE_IEEE802_11: u16 = 105;
const OPTION_IF_TSRESOL: u16 = 9;

pub(super) fn write_capture(
    path: &Path,
    packets: &[CapturedPacket],
    host_anchor_micros: u64,
) -> io::Result<()> {
    let mut output = File::create(path)?;
    write_section(&mut output)?;
    let snaplen = packets
        .iter()
        .map(|packet| packet.bytes.len())
        .max()
        .unwrap_or(0)
        .max(1) as u32;
    write_interface(&mut output, snaplen)?;
    let target_anchor = packets
        .first()
        .map_or(0, |packet| packet.dequeued_at_micros);
    for packet in packets {
        let timestamp = host_anchor_micros
            .saturating_add(packet.dequeued_at_micros.saturating_sub(target_anchor));
        write_packet(&mut output, packet, timestamp)?;
    }
    output.flush()
}

fn write_section(output: &mut impl Write) -> io::Result<()> {
    let mut body = Vec::with_capacity(16);
    push_u32(&mut body, BYTE_ORDER_MAGIC);
    push_u16(&mut body, 1);
    push_u16(&mut body, 0);
    body.extend_from_slice(&u64::MAX.to_le_bytes());
    write_block(output, SECTION_HEADER_BLOCK, &body)
}

fn write_interface(output: &mut impl Write, snaplen: u32) -> io::Result<()> {
    let mut body = Vec::with_capacity(20);
    push_u16(&mut body, LINKTYPE_IEEE802_11);
    push_u16(&mut body, 0);
    push_u32(&mut body, snaplen);
    push_u16(&mut body, OPTION_IF_TSRESOL);
    push_u16(&mut body, 1);
    body.push(6); // decimal microseconds
    pad_to_word(&mut body);
    push_u16(&mut body, 0);
    push_u16(&mut body, 0);
    write_block(output, INTERFACE_DESCRIPTION_BLOCK, &body)
}

fn write_packet(
    output: &mut impl Write,
    packet: &CapturedPacket,
    timestamp_micros: u64,
) -> io::Result<()> {
    let captured_length = u32::try_from(packet.bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "captured packet is too large"))?;
    if packet.logical_length < captured_length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "logical packet length is smaller than captured data",
        ));
    }
    let mut body = Vec::with_capacity(20 + packet.bytes.len() + 4);
    push_u32(&mut body, 0);
    push_u32(&mut body, (timestamp_micros >> 32) as u32);
    push_u32(&mut body, timestamp_micros as u32);
    push_u32(&mut body, captured_length);
    push_u32(&mut body, packet.logical_length);
    body.extend_from_slice(&packet.bytes);
    pad_to_word(&mut body);
    write_block(output, ENHANCED_PACKET_BLOCK, &body)
}

fn write_block(output: &mut impl Write, kind: u32, body: &[u8]) -> io::Result<()> {
    debug_assert_eq!(body.len() % 4, 0);
    let total_length = u32::try_from(12 + body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "PCAPNG block is too large"))?;
    output.write_all(&kind.to_le_bytes())?;
    output.write_all(&total_length.to_le_bytes())?;
    output.write_all(body)?;
    output.write_all(&total_length.to_le_bytes())
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn pad_to_word(output: &mut Vec<u8>) {
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
}

#[cfg(test)]
mod tests;
