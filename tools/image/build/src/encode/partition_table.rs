//! The app/data partition tables consumed by the image pipeline.
//!
//! Format facts only; this is an independent implementation, not an ESP-IDF port.
//! SOURCE: ESP-IDF 4d59230ddff16327812782151ef0afef202dc6d7 (the esp32s31
//! artifacts.toml pin), components/partition_table/gen_esp32part.py:
//! PartitionDefinition.STRUCT_FORMAT, FLAGS, TYPES, SUBTYPES, ALIGNMENT,
//! parse_int and PartitionTable.{from_binary,to_binary}.
//! SOURCE: the same pin, components/bootloader_support/include/esp_flash_partitions.h:
//! esp_partition_info_t, ESP_PARTITION_MAGIC, ESP_PARTITION_MAGIC_MD5,
//! ESP_PARTITION_MD5_OFFSET, ESP_PARTITION_TABLE_MAX_LEN and PART_FLAG_*.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use md5::{Digest as _, Md5};
use oer_wire::{Reader, Writer};

// SOURCE: gen_esp32part.py, MAX_PARTITION_LENGTH, MD5_PARTITION_BEGIN and
// PartitionDefinition.STRUCT_FORMAT; esp_flash_partitions.h, ESP_PARTITION_MAGIC*.
const RECORD_BYTES: usize = 32;
const TABLE_BYTES: usize = 0xc00;
const MAGIC: u16 = 0x50aa;
const MD5_MAGIC: u16 = 0xebeb;
const MAX_PARTITIONS: usize = TABLE_BYTES / RECORD_BYTES - 2; // MD5 and end marker.

// SOURCE: esp_flash_partitions.h, esp_partition_info_t; gen_esp32part.py,
// PartitionDefinition.STRUCT_FORMAT specifies little-endian byte order.
oer_wire::bitfield! {
    struct Header[12] (little) {
        magic, with_magic: u16 = 0..16;
        kind, with_kind: u8 = 16..24;
        subtype, with_subtype: u8 = 24..32;
        offset, with_offset: u32 = 32..64;
        size, with_size: u32 = 64..96;
    }
}

// SOURCE: esp_flash_partitions.h, PART_FLAG_ENCRYPTED and PART_FLAG_READONLY.
// Unknown bits remain in the value when a binary table is read and written.
oer_wire::bitfield! {
    struct Flags[4] (little) {
        encrypted, with_encrypted: bool = 0..1;
        readonly, with_readonly: bool = 1..2;
    }
}

#[derive(Debug)]
pub(super) enum Error {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Csv {
        line: usize,
        source: Box<Error>,
    },
    Columns,
    Number(String),
    Overflow,
    UnknownType(String),
    UnknownSubtype(String),
    UnknownFlag(String),
    Label,
    ZeroSize,
    Alignment,
    Overlap,
    DuplicateLabel,
    Empty,
    Length(usize),
    TooMany,
    Wire(oer_wire::Error),
    Magic(u16),
    Md5Record,
    Md5Mismatch,
    MissingEnd,
    TrailingData,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Csv { line, source } => write!(f, "partition CSV line {line}: {source}"),
            Self::Columns => f.write_str("expected name,type,subtype,offset,size[,flags]"),
            Self::Number(value) => write!(f, "invalid partition number `{value}`"),
            Self::Overflow => f.write_str("partition number or end address overflows"),
            Self::UnknownType(value) => write!(f, "unsupported partition type `{value}`"),
            Self::UnknownSubtype(value) => write!(f, "unsupported partition subtype `{value}`"),
            Self::UnknownFlag(value) => write!(f, "unknown partition flag `{value}`"),
            Self::Label => {
                f.write_str("partition label must be nonempty UTF-8, at most 16 bytes, without NUL")
            }
            Self::ZeroSize => f.write_str("partition size must be nonzero"),
            Self::Alignment => f.write_str("partition offset is not aligned for its type"),
            Self::Overlap => f.write_str("partitions overlap"),
            Self::DuplicateLabel => f.write_str("partition labels are not unique"),
            Self::Empty => f.write_str("partition table is empty"),
            Self::Length(length) => write!(
                f,
                "partition table length {length} must be a nonzero multiple of 32, at most 0xc00"
            ),
            Self::TooMany => f.write_str("partition table has too many entries"),
            Self::Wire(error) => write!(f, "partition field: {error:?}"),
            Self::Magic(magic) => write!(f, "invalid partition magic 0x{magic:04x}"),
            Self::Md5Record => f.write_str("invalid or misplaced partition MD5 record"),
            Self::Md5Mismatch => f.write_str("partition MD5 mismatch"),
            Self::MissingEnd => f.write_str("partition table has no erased end marker"),
            Self::TrailingData => f.write_str("non-erased data after partition table end marker"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Csv { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<oer_wire::Error> for Error {
    fn from(error: oer_wire::Error) -> Self {
        Self::Wire(error)
    }
}

type TableResult<T> = std::result::Result<T, Error>;

// SOURCE: esp_flash_partitions.h, PART_TYPE_APP and PART_TYPE_DATA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    App,
    Data,
}

impl Kind {
    fn from_raw(raw: u8) -> TableResult<Self> {
        match raw {
            0 => Ok(Self::App),
            1 => Ok(Self::Data),
            _ => Err(Error::UnknownType(format!("0x{raw:02x}"))),
        }
    }
    fn raw(self) -> u8 {
        match self {
            Self::App => 0,
            Self::Data => 1,
        }
    }
    // SOURCE: gen_esp32part.py, ALIGNMENT: app offsets 64 KiB, data 4 KiB.
    fn alignment(self) -> u32 {
        match self {
            Self::App => 0x10000,
            Self::Data => 0x1000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    kind: Kind,
    subtype: u8,
    offset: u32,
    size: u32,
    label: [u8; 16],
    flags: Flags,
}

impl Entry {
    fn name(&self) -> TableResult<&str> {
        let length = self
            .label
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.label.len());
        let name = std::str::from_utf8(self.label.get(..length).ok_or(Error::Label)?)
            .map_err(|_| Error::Label)?;
        if name.is_empty() {
            return Err(Error::Label);
        }
        Ok(name)
    }
    fn end(&self) -> TableResult<u32> {
        self.offset.checked_add(self.size).ok_or(Error::Overflow)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Table {
    entries: Vec<Entry>,
}

impl Table {
    pub(super) fn read_csv(path: &Path) -> TableResult<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::from_csv(&text)
    }

    fn from_csv(text: &str) -> TableResult<Self> {
        let mut entries = Vec::new();
        for (line, text) in text.lines().enumerate() {
            let text = text.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            if entries.len() == MAX_PARTITIONS {
                return Err(Error::TooMany);
            }
            entries.push(csv_entry(text).map_err(|source| Error::Csv {
                line: line + 1,
                source: Box::new(source),
            })?);
        }
        let table = Self { entries };
        table.validate()?;
        Ok(table)
    }

    pub(super) fn from_binary(bytes: &[u8]) -> TableResult<Self> {
        if bytes.is_empty()
            || bytes.len() > TABLE_BYTES
            || !bytes.len().is_multiple_of(RECORD_BYTES)
        {
            return Err(Error::Length(bytes.len()));
        }
        let mut input = Reader::new(bytes);
        let mut entries = Vec::new();
        let mut digest = Md5::new();
        let mut has_md5 = false;
        while !input.is_empty() {
            let record = input.array::<RECORD_BYTES>()?;
            if record.iter().all(|byte| *byte == 0xff) {
                if input.remaining().iter().any(|byte| *byte != 0xff) {
                    return Err(Error::TrailingData);
                }
                let table = Self { entries };
                table.validate()?;
                return Ok(table);
            }
            let mut fields = Reader::new(record);
            let header = Header::from_bytes(fields.read()?);
            let magic = header.magic();
            if magic == MD5_MAGIC {
                let mut fields = Reader::new(record);
                fields.u16_le()?;
                if has_md5
                    || entries.is_empty()
                    || fields.take(14)?.iter().any(|byte| *byte != 0xff)
                {
                    return Err(Error::Md5Record);
                }
                if fields.array::<16>()? != digest.clone().finalize().as_slice() {
                    return Err(Error::Md5Mismatch);
                }
                has_md5 = true;
                continue;
            }
            if has_md5 {
                return Err(Error::Md5Record);
            }
            if magic != MAGIC {
                return Err(Error::Magic(magic));
            }
            if entries.len() == MAX_PARTITIONS {
                return Err(Error::TooMany);
            }
            entries.push(Entry {
                kind: Kind::from_raw(header.kind())?,
                subtype: header.subtype(),
                offset: header.offset(),
                size: header.size(),
                label: fields.read()?,
                flags: Flags::from_bytes(fields.read()?),
            });
            digest.update(record);
        }
        Err(Error::MissingEnd)
    }

    pub(super) fn to_binary(&self) -> TableResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = vec![0xff; TABLE_BYTES];
        let mut digest = Md5::new();
        let mut output = Writer::new(&mut bytes);
        for entry in &self.entries {
            let header = Header::from_bytes([0; 12])
                .with_magic(MAGIC)?
                .with_kind(entry.kind.raw())?
                .with_subtype(entry.subtype)?
                .with_offset(entry.offset)?
                .with_size(entry.size)?;
            let mut record = [0; RECORD_BYTES];
            let mut fields = Writer::new(&mut record);
            fields.write(&header.to_bytes())?;
            fields.write(&entry.label)?;
            fields.write(&entry.flags.to_bytes())?;
            digest.update(record);
            output.write(&record)?;
        }
        output.u16_le(MD5_MAGIC)?;
        output.write(&[0xff; 14])?;
        output.write(&digest.finalize())?;
        Ok(bytes)
    }

    pub(super) fn partitions(&self) -> TableResult<Vec<super::Partition>> {
        self.entries
            .iter()
            .map(|entry| {
                Ok(super::Partition {
                    name: entry.name()?.to_owned(),
                    offset: entry.offset,
                    size: entry.size,
                    application: entry.kind == Kind::App,
                    // SOURCE: esp_flash_partitions.h, PART_SUBTYPE_DATA_OTA.
                    otadata: entry.kind == Kind::Data && entry.subtype == 0,
                })
            })
            .collect()
    }

    fn validate(&self) -> TableResult<()> {
        if self.entries.is_empty() {
            return Err(Error::Empty);
        }
        if self.entries.len() > MAX_PARTITIONS {
            return Err(Error::TooMany);
        }
        for (index, entry) in self.entries.iter().enumerate() {
            entry.name()?;
            let end = entry.end()?;
            if entry.size == 0 {
                return Err(Error::ZeroSize);
            }
            if !entry.offset.is_multiple_of(entry.kind.alignment()) {
                return Err(Error::Alignment);
            }
            for previous in self.entries.iter().take(index) {
                if entry.offset < previous.end()? && previous.offset < end {
                    return Err(Error::Overlap);
                }
                if entry.name()? == previous.name()? {
                    return Err(Error::DuplicateLabel);
                }
            }
        }
        Ok(())
    }
}

fn csv_entry(text: &str) -> TableResult<Entry> {
    let fields: Vec<_> = text.split(',').map(str::trim).collect();
    let (name, kind, subtype, offset, size, flags) = match fields.as_slice() {
        [name, kind, subtype, offset, size] => (*name, *kind, *subtype, *offset, *size, ""),
        [name, kind, subtype, offset, size, flags] => {
            (*name, *kind, *subtype, *offset, *size, *flags)
        }
        _ => return Err(Error::Columns),
    };
    if name.is_empty() || name.len() > 16 || name.contains('\0') {
        return Err(Error::Label);
    }
    let mut label = [0; 16];
    Writer::new(&mut label).write(name.as_bytes())?;
    let kind = match kind {
        "app" => Kind::App,
        "data" => Kind::Data,
        _ => Kind::from_raw(
            u8::try_from(number(kind).map_err(|error| match error {
                Error::Number(_) => Error::UnknownType(kind.to_owned()),
                other => other,
            })?)
            .map_err(|_| Error::Overflow)?,
        )?,
    };
    let mut flag_bits = Flags::from_bytes([0; 4]);
    for flag in flags.split(':').filter(|flag| !flag.is_empty()) {
        flag_bits = match flag {
            "encrypted" => flag_bits.with_encrypted(true),
            "readonly" => flag_bits.with_readonly(true),
            _ => return Err(Error::UnknownFlag(flag.to_owned())),
        };
    }
    Ok(Entry {
        kind,
        subtype: subtype_number(kind, subtype)?,
        offset: number(offset)?,
        size: number(size)?,
        label,
        flags: flag_bits,
    })
}

// SOURCE: gen_esp32part.py, SUBTYPES and MIN_PARTITION_SUBTYPE_APP_OTA:
// the names used by the image pipeline, plus numeric subtypes retained verbatim.
fn subtype_number(kind: Kind, text: &str) -> TableResult<u8> {
    match (kind, text) {
        (Kind::App, "factory") | (Kind::Data, "ota") => Ok(0),
        (Kind::Data, "phy") => Ok(1),
        (Kind::Data, "nvs") => Ok(2),
        (Kind::App, text) if text.starts_with("ota_") => {
            let slot = text
                .strip_prefix("ota_")
                .ok_or_else(|| Error::UnknownSubtype(text.to_owned()))?
                .parse::<u8>()
                .map_err(|_| Error::UnknownSubtype(text.to_owned()))?;
            if slot >= 16 {
                return Err(Error::UnknownSubtype(text.to_owned()));
            }
            Ok(0x10 + slot)
        }
        _ => u8::try_from(number(text).map_err(|error| match error {
            Error::Number(_) => Error::UnknownSubtype(text.to_owned()),
            other => other,
        })?)
        .map_err(|_| Error::Overflow),
    }
}

// SOURCE: gen_esp32part.py, parse_int: decimal/hex and K/M byte-size suffixes.
fn number(text: &str) -> TableResult<u32> {
    let (digits, multiplier) = match text.as_bytes().last() {
        Some(b'k' | b'K') => (
            text.strip_suffix(['k', 'K'])
                .ok_or_else(|| Error::Number(text.to_owned()))?,
            1024,
        ),
        Some(b'm' | b'M') => (
            text.strip_suffix(['m', 'M'])
                .ok_or_else(|| Error::Number(text.to_owned()))?,
            1024 * 1024,
        ),
        _ => (text, 1),
    };
    let (digits, radix) = match digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        Some(hex) => (hex, 16),
        None => (digits, 10),
    };
    let value = u32::from_str_radix(digits, radix).map_err(|error| match error.kind() {
        std::num::IntErrorKind::PosOverflow => Error::Overflow,
        _ => Error::Number(text.to_owned()),
    })?;
    value.checked_mul(multiplier).ok_or(Error::Overflow)
}

#[cfg(test)]
mod tests;
