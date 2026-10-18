//! Stage-two image packing, after the chip profile's `[staged.stage-two]`
//! header: the payload CRC-32 is computed with its own field read as zero.
//!
//! The profile data is the host's view of the bootstrap's contract
//! (`oer-espressif-staged-layout`'s `stage_two`), which this generic
//! pipeline does not link; both check the shared fixture
//! `platform/espressif/staged-layout/fixtures/stage-two.txt`, so they agree.
use crate::Result;
use oer_chip_profile::StageTwo;
use std::{fs, path::Path};

/// The little-endian word at byte `offset` of `bytes`.
fn word(bytes: &[u8], offset: u32) -> Option<u32> {
    let at = usize::try_from(offset).ok()?;
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// CRC-32 (IEEE 802.3) of `payload` with the four bytes at `crc_offset`
/// read as zero.
pub fn payload_crc32(payload: &[u8], crc_offset: u32) -> u32 {
    let start = crc_offset as usize;
    let field = start..start.saturating_add(4);
    let mut state = 0xffff_ffff_u32;
    for (index, byte) in payload.iter().enumerate() {
        let byte = if field.contains(&index) { 0 } else { *byte };
        state ^= u32::from(byte);
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(state & 1));
        }
    }
    !state
}

/// Store the payload CRC in the stage-two runtime image at `path` and
/// return it.
pub fn pack_runtime(header: &StageTwo, path: &Path) -> Result<u32> {
    // Every field must lie in the header: a profile that puts one outside
    // it is a contract error, never an index out of bounds.
    for (field, offset) in [
        ("magic", header.magic_offset),
        ("ABI version", header.abi_version_offset),
        ("header size", header.header_size_offset),
        ("payload CRC", header.crc_offset),
    ] {
        if offset
            .checked_add(4)
            .is_none_or(|end| end > header.header_bytes)
        {
            return Err(format!(
                "the stage-two contract puts its {field} field at {offset:#x}, outside its \
                 {}-byte header",
                header.header_bytes
            )
            .into());
        }
    }
    let mut bytes = fs::read(path)?;
    if bytes.len() < header.header_bytes as usize {
        return Err("runtime image is shorter than its header".into());
    }
    if word(&bytes, header.magic_offset) != Some(header.magic) {
        return Err("runtime image has the wrong stage-two magic".into());
    }
    if word(&bytes, header.abi_version_offset) != Some(header.abi_version)
        || word(&bytes, header.header_size_offset) != Some(header.header_bytes)
    {
        return Err("runtime image has an incompatible stage-two header".into());
    }
    let crc = payload_crc32(&bytes, header.crc_offset);
    let at = header.crc_offset as usize;
    bytes
        .get_mut(at..at + 4)
        .ok_or("the runtime image ends inside its CRC field")?
        .copy_from_slice(&crc.to_le_bytes());
    fs::write(path, bytes)?;
    let packed = fs::read(path)?;
    if word(&packed, header.crc_offset) != Some(crc)
        || payload_crc32(&packed, header.crc_offset) != crc
    {
        return Err("runtime CRC did not survive packing".into());
    }
    Ok(crc)
}
