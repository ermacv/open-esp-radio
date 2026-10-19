//! Borrowed data slices from the same prepared ELF owner as function views.
use super::*;
pub struct DataView<'a> {
    pub span: DataSpan,
    pub bytes: &'a [u8],
    pub relocations: &'a [FunctionRelocation],
    pub address_space: CodeAddressSpace,
    pub section_address: u64,
    pub max_relocation_width: u8,
}
impl PreparedObject<'_, '_> {
    pub fn validate_data_symbol(&self, occurrence: &ObjectId, symbol: &SymbolId) -> Result<()> {
        self.selected_symbol(occurrence, symbol)?;
        Ok(())
    }

    /// Resolve an exact physical span, including NOBITS, without materializing bytes.
    pub fn data_location(
        &mut self,
        occurrence: &ObjectId,
        selector: &DataSelector,
        control: &mut dyn RunControl,
    ) -> Result<DataLocation> {
        if self.occurrence.as_ref().is_some_and(|id| id != occurrence) {
            return Err(invalid("prepared data belongs to another occurrence"));
        }
        self.occurrence = Some(occurrence.clone());
        let (index, offset, length) = match selector {
            DataSelector::Section {
                section,
                offset,
                length,
            } => (object::SectionIndex(*section as usize), *offset, *length),
            DataSelector::Symbol { symbol, length } => {
                let symbol = self.selected_symbol(occurrence, symbol)?;
                let index = symbol
                    .section_index()
                    .ok_or_else(|| invalid("data symbol has no defined section"))?;
                let section = self.file.section_by_index(index).map_err(parse)?;
                let offset = symbol
                    .address()
                    .checked_sub(section.address())
                    .ok_or_else(|| invalid("symbol precedes section"))?;
                let length = length.unwrap_or(symbol.size());
                if symbol.size() != 0 && length > symbol.size() {
                    return Err(invalid("requested bytes exceed declared symbol size"));
                }
                (index, offset, length)
            }
            DataSelector::Image { address, length } => {
                if self.file.kind() != object::ObjectKind::Executable {
                    return Err(invalid("image addresses require executable ELF"));
                }
                let end = address
                    .checked_add(*length)
                    .ok_or_else(|| invalid("image range overflow"))?;
                let mut found = None;
                for s in self.file.sections() {
                    control.checkpoint(1)?;
                    if matches!(s.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0)
                        && *address >= s.address()
                        && s.address().checked_add(s.size()).is_some_and(|n| end <= n)
                    {
                        if found.is_some() {
                            return Err(invalid("ambiguous image data range"));
                        }
                        found = Some((s.index(), address - s.address(), *length));
                    }
                }
                found.ok_or_else(|| invalid("image range has no exact section"))?
            }
        };
        if length == 0 {
            return Err(Error::new(
                ErrorCode::NeedsExtent,
                "data requires a nonempty explicit range or sized symbol",
            ));
        }
        let section = self.file.section_by_index(index).map_err(parse)?;
        if matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_COMPRESSED) != 0)
        {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "compressed data sections require an explicit decompression profile",
            ));
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= section.size())
            .ok_or_else(|| invalid("data range exceeds section"))?;
        let file_range = if let Some((file_offset, file_length)) = section.file_range() {
            if end > file_length {
                return Err(invalid("data range exceeds file backing"));
            }
            Some(CodeRange {
                start: file_offset
                    .checked_add(offset)
                    .ok_or_else(|| invalid("file range overflow"))?,
                length,
            })
        } else {
            None
        };
        let allocated = matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0);
        Ok(DataLocation {
            selector: selector.clone(),
            section: index.0 as u32,
            section_range: CodeRange {
                start: offset,
                length,
            },
            file_range,
            image_address: if self.file.kind() == object::ObjectKind::Executable && allocated {
                Some(
                    section
                        .address()
                        .checked_add(offset)
                        .ok_or_else(|| invalid("data address overflow"))?,
                )
            } else {
                None
            },
            section_writable: matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_WRITE) != 0),
        })
    }
    pub fn with_data<T>(
        &mut self,
        occurrence: &ObjectId,
        selector: &DataSelector,
        control: &mut dyn RunControl,
        consume: impl FnOnce(DataView<'_>, &mut dyn RunControl) -> Result<T>,
    ) -> Result<T> {
        let location = self.data_location(occurrence, selector, control)?;
        let index = object::SectionIndex(location.section as usize);
        let offset = location.section_range.start;
        let length = location.section_range.length;
        let end = offset + length; // Checked by data_location.
        let file_range = location
            .file_range
            .ok_or_else(|| invalid("data has no captured file bytes (NOBITS)"))?;
        self.prepare_section(index, occurrence, control)?;
        let section = self.file.section_by_index(index).map_err(parse)?;
        let data = section.data().map_err(parse)?;
        let bytes = data
            .get(
                usize::try_from(offset).map_err(|_| invalid("data offset overflow"))?
                    ..usize::try_from(end).map_err(|_| invalid("data end overflow"))?,
            )
            .ok_or_else(|| invalid("missing data bytes"))?;
        let file_start = file_range.start;
        let prepared = self.sections[index.0].as_ref().unwrap();
        let allocated = matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0);
        let mut writable = matches!(section.flags(), object::SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_WRITE) != 0);
        if let Some(image) = &self.image
            && allocated
        {
            writable |= image.data_range(
                section
                    .address()
                    .checked_add(offset)
                    .ok_or_else(|| invalid("data address overflow"))?,
                file_start,
                length,
                control,
            )?;
        }
        let mut max_relocation_width = 0;
        let mut overlapping_relocations = 0;
        let mut unknown_relocation_extents = 0;
        // Use structural widths supplied by the pinned ELF parser. Unknown
        // transformations may affect layout; their offset cannot prove exclusion.
        for (site, relocation) in section.relocations() {
            control.checkpoint(1)?;
            if relocation.kind() == object::RelocationKind::None {
                continue;
            }
            let width = relocation.size();
            if width == 0 || !width.is_multiple_of(8) {
                unknown_relocation_extents += 1;
                continue;
            }
            max_relocation_width = max_relocation_width.max(width / 8);
            let start = if self.file.kind() == object::ObjectKind::Executable {
                site.checked_sub(section.address())
                    .ok_or_else(|| invalid("relocation precedes target section"))?
            } else {
                site
            };
            let write_end = start
                .checked_add(u64::from(width / 8))
                .filter(|end| *end <= section.size())
                .ok_or_else(|| invalid("relocation write exceeds target section"))?;
            overlapping_relocations += u64::from(start < end && offset < write_end);
        }
        let span = DataSpan {
            selector: selector.clone(),
            section: index.0 as u32,
            section_range: CodeRange {
                start: offset,
                length,
            },
            file_range: CodeRange {
                start: file_start,
                length,
            },
            image_address: if self.file.kind() == object::ObjectKind::Executable && allocated {
                Some(
                    section
                        .address()
                        .checked_add(offset)
                        .ok_or_else(|| invalid("image address overflow"))?,
                )
            } else {
                None
            },
            writable,
            digest: ArtifactId::of_bytes_controlled(bytes, control)?,
            export_offset: 0,
            section_relocations: prepared.relocations.len() as u64,
            overlapping_relocations,
            unknown_relocation_extents,
        };
        consume(
            DataView {
                span,
                bytes,
                relocations: &prepared.relocations,
                address_space: if self.file.kind() == object::ObjectKind::Executable {
                    CodeAddressSpace::Image
                } else {
                    CodeAddressSpace::Section
                },
                section_address: section.address(),
                max_relocation_width,
            },
            control,
        )
    }
}
