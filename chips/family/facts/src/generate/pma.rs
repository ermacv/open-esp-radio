//! A secondary hart's PMA chain: the chip's `[hart-entry] pma` ranges as an
//! ascending top-of-range chain over the family's `[pma]` CSRs, rendered as
//! the straight-line assembly its generated entry runs.

use super::*;
use crate::{PmaAccess, PmaCsrs, PmaRange};

/// One programmed entry: its byte end (the address CSR holds it shifted) and
/// configuration. The entry covers the previous entry's end, or zero, up to
/// `end`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PmaEntry {
    /// The range's name, or `gap` for a range between two named ones.
    pub name: String,
    pub end: u32,
    pub config: u32,
}

/// The chain and the CSRs it is written through.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pma {
    pub csrs: PmaCsrs,
    pub entries: Vec<PmaEntry>,
}

/// A write the entry makes and reads back: a CSR and its value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmaWrite {
    /// The reported index: the entry number, plus 0x100 for an address CSR.
    pub index: u32,
    pub csr: u16,
    pub value: u32,
}

/// The reported index of an address CSR write is the entry plus this.
pub const PMA_ADDRESS_INDEX: u32 = 0x100;

impl Pma {
    /// The chain of `facts`: each range ascending and disjoint from the last,
    /// a gap between two ranges an enabled entry without access, every bound
    /// a multiple of the address granule, and at most the family's entries.
    pub fn of(facts: &Facts) -> Result<Self> {
        let missing = |what: &str| format!("{}: a secondary hart's PMA needs {what}", facts.id);
        let csrs = facts.pma.ok_or_else(|| missing("the family's `[pma]`"))?;
        let ranges = &facts
            .hart_entry
            .as_ref()
            .ok_or_else(|| missing("`[hart-entry]`"))?
            .pma;
        let memory = facts.memory.as_ref().ok_or_else(|| missing("`[memory]`"))?;
        let granule = 1_u32
            .checked_shl(u32::from(csrs.address_shift))
            .ok_or("the PMA address shift exceeds a word")?;
        let configuration = |access: PmaAccess, cached: bool| {
            let (read, write, execute) = access.bits();
            let mut config = csrs.top_of_range | csrs.enable;
            for (set, bit) in [
                (read, csrs.read),
                (write, csrs.write),
                (execute, csrs.execute),
                (!cached, csrs.noncacheable),
            ] {
                if set {
                    config |= bit;
                }
            }
            config
        };
        let mut entries = Vec::new();
        let mut previous = 0_u32;
        for range in ranges {
            let (start, end, access, cached) = resolve(range, memory)?;
            if start < previous || end <= start {
                return Err(format!(
                    "PMA range `{}` {start:#010x}..{end:#010x} is empty or not above the previous range",
                    range.name
                )
                .into());
            }
            if !start.is_multiple_of(granule) || !end.is_multiple_of(granule) {
                return Err(format!(
                    "PMA range `{}` bounds are not multiples of {granule} bytes",
                    range.name
                )
                .into());
            }
            if start > previous {
                entries.push(PmaEntry {
                    name: "gap".into(),
                    end: start,
                    config: configuration(PmaAccess::None, false),
                });
            }
            entries.push(PmaEntry {
                name: range.name.clone(),
                end,
                config: configuration(access, cached),
            });
            previous = end;
        }
        if entries.is_empty() || entries.len() > usize::from(csrs.entries) {
            return Err(format!(
                "{}: the PMA chain needs 1 to {} entries and has {}",
                facts.id,
                csrs.entries,
                entries.len()
            )
            .into());
        }
        Ok(Self { csrs, entries })
    }

    /// Every write in order: each entry disabled, then the chain ascending,
    /// an entry's address before its configuration.
    pub fn writes(&self) -> Vec<PmaWrite> {
        let mut writes = Vec::new();
        for entry in 0..self.csrs.entries {
            writes.push(PmaWrite {
                index: u32::from(entry),
                csr: self.csrs.cfg_csr.saturating_add(entry),
                value: self.csrs.disabled,
            });
        }
        for (entry, programmed) in (0_u16..).zip(&self.entries) {
            writes.push(PmaWrite {
                index: u32::from(entry) | PMA_ADDRESS_INDEX,
                csr: self.csrs.addr_csr.saturating_add(entry),
                value: programmed
                    .end
                    .checked_shr(u32::from(self.csrs.address_shift))
                    .unwrap_or(0),
            });
            writes.push(PmaWrite {
                index: u32::from(entry),
                csr: self.csrs.cfg_csr.saturating_add(entry),
                value: programmed.config,
            });
        }
        writes
    }

    /// The assembly macro `PMA_PROGRAM fail kind`: every write of
    /// [`Pma::writes`], each read back. A mismatch branches to `fail` with
    /// `a0` = `kind`, `a1` = the write's index, `t0` = the value written and
    /// `t1` = the value read; it touches no memory and no stack.
    pub fn assembly(&self) -> String {
        let mut text = String::from(
            "    // Generated from the chip's `[hart-entry] pma` and the family's `[pma]`.\n    .macro PMA_PROGRAM fail, kind\n    li a0, \\kind\n",
        );
        for write in self.writes() {
            let _ = writeln!(
                text,
                "    li a1, {index:#x}\n    li t0, {value:#010x}\n    csrw {csr:#x}, t0\n    csrr t1, {csr:#x}\n    bne t1, t0, \\fail",
                index = write.index,
                value = write.value,
                csr = write.csr,
            );
        }
        text.push_str("    .endm\n");
        text
    }
}

/// A range's bounds, access and cacheability.
fn resolve(range: &PmaRange, memory: &crate::Memory) -> Result<(u32, u32, PmaAccess, bool)> {
    match (
        &range.region,
        range.start,
        range.end,
        range.access,
        range.cached,
    ) {
        (Some(region), None, None, None, None) => {
            let (bounds, attributes) = if region == "internal-sram" {
                (memory.internal_sram, memory.attributes.get("sram").copied())
            } else {
                let mapped = memory
                    .regions()
                    .into_iter()
                    .find(|mapped| mapped.name == region)
                    .ok_or_else(|| {
                        format!(
                            "PMA range `{}` names no mapped region `{region}`",
                            range.name
                        )
                    })?;
                (mapped.region, Some(mapped.attributes))
            };
            let attributes = attributes.ok_or_else(|| {
                format!("PMA range `{}`: `{region}` has no attributes", range.name)
            })?;
            let access = match (attributes.writable, attributes.executable) {
                (true, true) => PmaAccess::Rwx,
                (true, false) => PmaAccess::Rw,
                (false, true) => PmaAccess::Rx,
                (false, false) => PmaAccess::R,
            };
            Ok((bounds.origin(), bounds.end(), access, attributes.cached))
        }
        (None, Some(start), Some(end), Some(access), Some(cached)) => {
            Ok((start, end, access, cached))
        }
        _ => Err(format!(
            "PMA range `{}` names either a `region` or its `start`, `end`, `access` and `cached`",
            range.name
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::tracked;

    #[test]
    fn the_chain_disables_every_entry_first_then_ascends_with_read_back() {
        let pma = Pma::of(&tracked("esp32s31")).expect("S31 PMA chain");
        let writes = pma.writes();
        let entries = usize::from(pma.csrs.entries);
        // Every entry disabled before any is programmed.
        for (entry, write) in writes.iter().take(entries).enumerate() {
            assert_eq!(write.index, entry as u32);
            assert_eq!(write.csr, pma.csrs.cfg_csr + entry as u16);
            assert_eq!(write.value, pma.csrs.disabled);
        }
        // Then each entry's address before its configuration, ascending.
        let programmed = &writes[entries..];
        assert_eq!(programmed.len(), 2 * pma.entries.len());
        let mut end = 0;
        for (entry, pair) in programmed.chunks_exact(2).enumerate() {
            assert_eq!(pair[0].csr, pma.csrs.addr_csr + entry as u16);
            assert_eq!(pair[1].csr, pma.csrs.cfg_csr + entry as u16);
            let address = pair[0].value << pma.csrs.address_shift;
            assert!(address > end, "ascending top-of-range chain");
            end = address;
            assert_ne!(pair[1].value & pma.csrs.enable, 0);
            assert_ne!(pair[1].value & pma.csrs.top_of_range, 0);
        }
        // Every write is read back and compared.
        let assembly = pma.assembly();
        assert_eq!(assembly.matches("csrw ").count(), writes.len());
        assert_eq!(assembly.matches("csrr t1, ").count(), writes.len());
        assert_eq!(assembly.matches("bne t1, t0, \\fail").count(), writes.len());
        let first_address = assembly
            .find(&format!("csrw {:#x},", pma.csrs.addr_csr))
            .expect("an address write");
        let last_disable = assembly
            .rfind(&format!(
                "li t0, {:#010x}\n    csrw {:#x}",
                pma.csrs.disabled,
                pma.csrs.cfg_csr + pma.csrs.entries - 1
            ))
            .expect("the last disable");
        assert!(last_disable < first_address);
    }

    #[test]
    fn the_s31_chain_names_psram_writable_and_cached_and_gaps_without_access() {
        let facts = tracked("esp32s31");
        let pma = Pma::of(&facts).expect("S31 PMA chain");
        let csrs = pma.csrs;
        let psram = facts.memory.as_ref().unwrap().psram.unwrap();
        let entry = |name: &str| {
            pma.entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("{name} entry"))
        };
        let psram_entry = entry("psram");
        assert_eq!(psram_entry.end, psram.end());
        for bit in [csrs.read, csrs.write, csrs.execute] {
            assert_ne!(psram_entry.config & bit, 0);
        }
        assert_eq!(psram_entry.config & csrs.noncacheable, 0);
        let flash = entry("flash");
        assert_eq!(flash.config & csrs.write, 0);
        assert_ne!(flash.config & csrs.execute, 0);
        assert_ne!(entry("internal-sram").config & csrs.noncacheable, 0);
        for gap in pma.entries.iter().filter(|entry| entry.name == "gap") {
            assert_eq!(gap.config & (csrs.read | csrs.write | csrs.execute), 0);
            assert_ne!(gap.config & csrs.enable, 0);
        }
        assert!(pma.entries.len() <= usize::from(csrs.entries));
    }

    #[test]
    fn overlapping_unaligned_mixed_or_excess_ranges_are_refused() {
        let base = tracked("esp32s31");
        let edit = |change: &dyn Fn(&mut Vec<PmaRange>)| {
            let mut facts = base.clone();
            change(&mut facts.hart_entry.as_mut().unwrap().pma);
            Pma::of(&facts)
        };
        assert!(edit(&|ranges| ranges.reverse()).is_err());
        assert!(edit(&|ranges| ranges[0].start = Some(0x2010_0001)).is_err());
        assert!(edit(&|ranges| ranges[1].start = Some(0)).is_err());
        assert!(edit(&|ranges| ranges[1].region = Some("nowhere".into())).is_err());
        assert!(
            edit(&|ranges| {
                let last = ranges.last().unwrap().clone();
                for step in 1..12 {
                    let mut range = last.clone();
                    range.region = None;
                    range.start = Some(0x6000_0000 + step * 0x100_0000);
                    range.end = Some(0x6000_0000 + step * 0x100_0000 + 0x10);
                    range.access = Some(PmaAccess::Rw);
                    range.cached = Some(false);
                    ranges.push(range);
                }
            })
            .is_err()
        );
        let mut missing = base.clone();
        missing.pma = None;
        assert!(Pma::of(&missing).is_err());
    }
}
