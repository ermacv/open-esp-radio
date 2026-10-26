use super::*;
use oer_espressif_staged_layout::{Region, stage_two};
use std::vec::Vec;

const LAYOUT: Layout = Layout {
    sram: Region::new(0x1000, 0x1000).expect("fixture region"),
    second_stage_loader_start: 0x1800,
    flash_xip: Region::new(0x4000_0000, 0x1000).expect("fixture region"),
    psram: Region::new(0x5000_0000, 1024).expect("fixture region"),
    bootstrap_psram_bytes: 512,
    mmu_page_bytes: 512,
    image_segment_header_bytes: 16,
    handoff_sram_margin_bytes: 16,
    retained: Region::new(0x2000, 16).expect("fixture region"),
    retained_reserved_tail: 0,
    cpu0_task_stack_bytes: 16,
    irq_stack_bytes: 16,
    harts: 1,
    flash_tuning_reference_bytes: 0,
    rom_script: "",
};

fn image() -> [u8; 64] {
    let mut payload = [0xa5; 64];
    let load_address = LAYOUT.runtime_psram().expect("runtime region").origin();
    let header = RuntimeHeader {
        magic: stage_two::MAGIC,
        abi_version: stage_two::ABI_VERSION,
        load_address,
        entry: load_address + 48,
        payload_end: load_address + 64,
        bss_start: load_address + 72,
        bss_end: load_address + 88,
        header_size: stage_two::HEADER_BYTES as u32,
        text_start: load_address + 48,
        text_end: load_address + 64,
        payload_crc32: 0,
    };
    payload[..stage_two::HEADER_BYTES].copy_from_slice(&header.to_le_bytes());
    let crc = stage_two::payload_crc32(&payload);
    payload[stage_two::CRC_OFFSET..stage_two::CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    payload
}

#[test]
fn stage_copies_payload_clears_only_bss_and_reports_in_order() {
    let payload = image();
    let mut words = [0x5a5a_5a5a_u32; 256];
    let psram: &mut [u8] = bytemuck::cast_slice_mut(&mut words);
    let expected_address = psram.as_ptr() as usize + 512;
    let mut progress = Vec::new();
    let staged = stage(&LAYOUT, &payload, psram, |step| progress.push(step)).expect("valid image");
    assert_eq!(staged.load_address(), expected_address);
    assert_eq!(staged.payload_len(), payload.len());
    assert_eq!(staged.entry(), LAYOUT.psram.origin() as usize + 512 + 48);
    assert_eq!(staged.payload, payload);
    // End the payload loan before inspecting the rest of the region.
    let _ = staged;
    assert_eq!(&psram[512..576], &payload);
    assert!(psram[576..584].iter().all(|byte| *byte == 0x5a));
    assert!(psram[584..600].iter().all(|byte| *byte == 0));
    assert!(psram[600..].iter().all(|byte| *byte == 0x5a));
    assert!(psram[..0x100].iter().all(|byte| *byte == 0x5a));
    assert_eq!(&psram[0x100..0x104], &0x31a5_c33c_u32.to_ne_bytes());
    assert_eq!(&psram[0x104..0x108], &0xc35a_3cc3_u32.to_ne_bytes());
    assert!(psram[0x108..512].iter().all(|byte| *byte == 0x5a));
    assert_eq!(
        progress,
        [
            StageProgress::Psram,
            StageProgress::SourceCrc,
            StageProgress::DestinationCrc
        ]
    );
}

#[test]
fn truncated_payloads_return_typed_errors_without_touching_the_destination() {
    let payload = image();
    for (length, expected) in [
        (stage_two::HEADER_BYTES - 1, StageError::ShortHeader),
        (63, StageError::PayloadLength),
    ] {
        let mut words = [0x5a5a_5a5a_u32; 256];
        let psram = bytemuck::cast_slice_mut(&mut words);
        let error =
            stage(&LAYOUT, &payload[..length], psram, |_| {}).expect_err("truncated payload");
        assert_eq!(error, expected);
        assert!(psram[512..].iter().all(|byte| *byte == 0x5a));
    }
}

#[test]
fn a_bad_source_crc_is_a_typed_error_before_copy_or_bss_zeroing() {
    let mut payload = image();
    let expected = payload_crc32(&payload);
    payload[63] ^= 1;
    let observed = payload_crc32(&payload);
    let mut words = [0x5a5a_5a5a_u32; 256];
    let psram = bytemuck::cast_slice_mut(&mut words);
    let mut progress = Vec::new();
    assert_eq!(
        stage(&LAYOUT, &payload, psram, |step| progress.push(step)).expect_err("bad CRC"),
        StageError::SourceCrc { expected, observed }
    );
    assert!(psram[512..].iter().all(|byte| *byte == 0x5a));
    assert_eq!(progress, [StageProgress::Psram]);
}

#[test]
fn crc_reads_the_slice_and_ignores_its_stored_checksum() {
    assert_eq!(payload_crc32(b"123456789"), 0xcbf4_3926);
    let mut payload = image();
    let expected = payload_crc32(&payload);
    payload[stage_two::CRC_OFFSET..stage_two::CRC_OFFSET + 4].fill(0xff);
    assert_eq!(payload_crc32(&payload), expected);
}

#[test]
fn malformed_header_ranges_are_rejected_before_copying() {
    for change in [
        |header: &mut RuntimeHeader| header.load_address = u32::MAX,
        |header: &mut RuntimeHeader| header.payload_end = header.load_address,
        |header: &mut RuntimeHeader| header.bss_start = header.payload_end - 1,
        |header: &mut RuntimeHeader| header.bss_end = LAYOUT.psram.end() + 1,
        |header: &mut RuntimeHeader| header.entry = header.text_end,
        |header: &mut RuntimeHeader| header.text_start = header.load_address,
    ] {
        let mut payload = image();
        let mut header = RuntimeHeader::from_le_bytes(&payload).expect("header");
        change(&mut header);
        payload[..stage_two::HEADER_BYTES].copy_from_slice(&header.to_le_bytes());
        let mut words = [0x5a5a_5a5a_u32; 256];
        let psram = bytemuck::cast_slice_mut(&mut words);
        assert_eq!(
            stage(&LAYOUT, &payload, psram, |_| {}).expect_err("bad header"),
            StageError::Header
        );
        assert!(psram[512..].iter().all(|byte| *byte == 0x5a));
    }
}

#[test]
fn a_short_or_unaligned_psram_region_is_rejected_without_writing() {
    let payload = image();
    let mut words = [0x5a5a_5a5a_u32; 257];
    let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut words);
    assert_eq!(
        stage(&LAYOUT, &payload, &mut bytes[..1023], |_| {}).expect_err("short region"),
        StageError::PsramRegion
    );
    assert_eq!(
        stage(&LAYOUT, &payload, &mut bytes[1..1025], |_| {}).expect_err("unaligned region"),
        StageError::PsramRegion
    );
    assert!(bytes.iter().all(|byte| *byte == 0x5a));
}
#[test]
fn boot_failure_records_retain_the_typed_density_in_bytes() {
    use oer_espressif_boot::{BootFailure, psram::PsramFailure};

    assert_eq!(
        std::format!(
            "{}",
            super::BootFailureRecord(BootFailure::Psram(PsramFailure::DensityMismatch {
                found: 8 * 1024 * 1024
            }))
        ),
        "OER_BOOT bootstrap=FAIL reason=psram-density-mismatch found=8388608\r\n"
    );
    assert_eq!(
        std::format!(
            "{}",
            super::BootFailureRecord(BootFailure::Psram(PsramFailure::UnsupportedDensity(2)))
        ),
        "OER_BOOT bootstrap=FAIL reason=psram-density-unknown code=2\r\n"
    );
}
