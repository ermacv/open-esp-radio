//! The stage-two runtime as the bootstrap embeds it: the runtime ELF's load
//! image from its header to `__runtime_payload_end`, with the payload CRC
//! stored in the header (`oer-platform-staged-layout`'s `stage_two`).

use object::{Object as _, ObjectSymbol as _, read::elf::ElfFile32, read::elf::ProgramHeader as _};
use oer_platform_staged_layout::stage_two::{CRC_OFFSET, Header, payload_crc32};

use crate::Result;

/// The runtime linker script's bounds of the load image.
pub const IMAGE_START_SYMBOL: &str = "__runtime_image_start";
pub const PAYLOAD_END_SYMBOL: &str = "__runtime_payload_end";

/// One loaded part of an ELF: its load (physical) address and file bytes.
#[derive(Clone, Copy, Debug)]
pub struct LoadSegment<'a> {
    pub address: u32,
    pub data: &'a [u8],
}

/// The packed runtime and the header it carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packed {
    pub bytes: Vec<u8>,
    pub header: Header,
}

/// Pack the runtime ELF `elf`.
pub fn pack_elf(elf: &[u8]) -> Result<Packed> {
    let file = ElfFile32::<object::LittleEndian>::parse(elf)?;
    let symbol = |name: &str| -> Result<u32> {
        let symbol = file
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .ok_or_else(|| format!("the runtime ELF has no `{name}`"))?;
        Ok(u32::try_from(symbol.address())?)
    };
    let start = symbol(IMAGE_START_SYMBOL)?;
    let end = symbol(PAYLOAD_END_SYMBOL)?;
    let endian = file.endian();
    let mut segments = Vec::new();
    for header in file.elf_program_headers() {
        if header.p_type(endian) != object::elf::PT_LOAD {
            continue;
        }
        let data = header
            .data(endian, elf)
            .map_err(|()| "a runtime load segment lies outside the ELF")?;
        if data.is_empty() {
            continue;
        }
        segments.push(LoadSegment {
            address: header.p_paddr(endian),
            data,
        });
    }
    pack(&segments, start, end)
}

/// Flatten `segments` into the image `start..end`, check its stage-two
/// header and store the payload CRC.
pub fn pack(segments: &[LoadSegment<'_>], start: u32, end: u32) -> Result<Packed> {
    let length = end
        .checked_sub(start)
        .ok_or("the runtime payload ends before it starts")?;
    let mut bytes = vec![0_u8; usize::try_from(length)?];
    for segment in segments {
        let offset = segment
            .address
            .checked_sub(start)
            .filter(|offset| *offset < length)
            .ok_or_else(|| {
                format!(
                    "a runtime load segment at {:#010x} lies outside the image {start:#010x}..{end:#010x}",
                    segment.address
                )
            })?;
        let offset = usize::try_from(offset)?;
        let target = bytes
            .get_mut(offset..offset + segment.data.len())
            .ok_or_else(|| {
                format!(
                    "the runtime load segment at {:#010x} runs past the payload end {end:#010x}",
                    segment.address
                )
            })?;
        target.copy_from_slice(segment.data);
    }
    let mut header =
        Header::from_le_bytes(&bytes).ok_or("the runtime image is shorter than its header")?;
    if !header.is_compatible() {
        return Err("the runtime image has no compatible stage-two header".into());
    }
    if header.load_address != start || header.payload_end != end {
        return Err(format!(
            "the stage-two header loads {:#010x}..{:#010x}, the image is {start:#010x}..{end:#010x}",
            header.load_address, header.payload_end
        )
        .into());
    }
    if !(header.text_start..header.text_end).contains(&header.entry) {
        return Err(format!(
            "the runtime entry {:#010x} lies outside its text {:#010x}..{:#010x}",
            header.entry, header.text_start, header.text_end
        )
        .into());
    }
    let crc = payload_crc32(&bytes);
    bytes
        .get_mut(CRC_OFFSET..CRC_OFFSET + 4)
        .ok_or("the runtime image ends inside its CRC field")?
        .copy_from_slice(&crc.to_le_bytes());
    header.payload_crc32 = crc;
    Ok(Packed { bytes, header })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_platform_staged_layout::stage_two::{ABI_VERSION, HEADER_BYTES, MAGIC};

    fn header(load_address: u32, payload_end: u32) -> Header {
        Header {
            magic: MAGIC,
            abi_version: ABI_VERSION,
            load_address,
            entry: load_address + 0x40,
            payload_end,
            bss_start: payload_end,
            bss_end: payload_end,
            header_size: HEADER_BYTES as u32,
            text_start: load_address + 0x40,
            text_end: load_address + 0x80,
            payload_crc32: 0,
        }
    }

    #[test]
    fn flattens_segments_by_load_address_and_stores_the_crc() {
        let start = 0x5001_0000;
        let end = start + 0x100;
        let head = header(start, end).to_le_bytes();
        let text = [0x13_u8; 0x20];
        let data = [0xa5_u8; 4];
        let packed = pack(
            &[
                LoadSegment {
                    address: start + 0x40,
                    data: &text,
                },
                LoadSegment {
                    address: start,
                    data: &head,
                },
                LoadSegment {
                    address: end - 4,
                    data: &data,
                },
            ],
            start,
            end,
        )
        .unwrap();
        assert_eq!(packed.bytes.len(), 0x100);
        assert_eq!(&packed.bytes[0x40..0x60], &text);
        assert!(packed.bytes[0x60..0xfc].iter().all(|byte| *byte == 0));
        assert_eq!(&packed.bytes[0xfc..], &data);
        let stored =
            u32::from_le_bytes(packed.bytes[CRC_OFFSET..CRC_OFFSET + 4].try_into().unwrap());
        assert_eq!(stored, packed.header.payload_crc32);
        assert_eq!(stored, payload_crc32(&packed.bytes));
        assert_eq!(Header::from_le_bytes(&packed.bytes), Some(packed.header));
    }

    #[test]
    fn refuses_segments_outside_the_image() {
        let start = 0x5001_0000;
        let head = header(start, start + 0x100).to_le_bytes();
        let stray = [0_u8; 8];
        for address in [start - 4, start + 0xfc, start + 0x100] {
            let error = pack(
                &[
                    LoadSegment {
                        address: start,
                        data: &head,
                    },
                    LoadSegment {
                        address,
                        data: &stray,
                    },
                ],
                start,
                start + 0x100,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("runtime load segment"),
                "{error}"
            );
        }
    }

    #[test]
    fn refuses_a_header_of_another_image_or_contract() {
        let start = 0x5001_0000;
        let mut other = header(start, start + 0x200);
        let mut bytes = other.to_le_bytes();
        let segment = |bytes: &[u8; HEADER_BYTES]| {
            pack(
                &[LoadSegment {
                    address: start,
                    data: bytes,
                }],
                start,
                start + 0x100,
            )
            .unwrap_err()
            .to_string()
        };
        assert!(segment(&bytes).contains("the stage-two header loads"));
        other.payload_end = start + 0x100;
        other.abi_version += 1;
        bytes = other.to_le_bytes();
        assert!(segment(&bytes).contains("no compatible stage-two header"));
    }
}
