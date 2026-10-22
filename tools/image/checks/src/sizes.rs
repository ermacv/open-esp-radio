//! Image size measurements. These observe the checks' existing analysis;
//! they neither run an analysis nor decide a check's verdict.
use crate::{Result, stack::Image};
use oer_analysis_riscv_stack::Bound;
use oer_chip_profile::{Profile, policy::Storage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod types;

/// Aggregate and array layouts smaller than this are omitted from the inventory.
pub const TYPE_MIN_BYTES: u64 = 256;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measurements {
    pub sections: BTreeMap<String, Section>,
    /// Executable section bytes and all allocated RAM bytes, by memory kind.
    pub totals: BTreeMap<String, Totals>,
    pub allocated_stacks: BTreeMap<String, u64>,
    pub bounds: BTreeMap<String, StackBound>,
    /// All aliases of every compiler frame record. Multiple implementations
    /// of the same demangled name retain their individual sizes, sorted.
    pub frames: BTreeMap<String, Vec<u64>>,
    pub types: BTreeMap<String, u64>,
    pub resources: Resources,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub memory: String,
    pub executable: bool,
    pub writable: bool,
    pub address: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    pub code: u64,
    pub ram: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub work_units: u64,
    pub peak_memory_bytes: u64,
    pub work_limit: u64,
    pub memory_limit_bytes: u64,
    pub timeout_seconds: u64,
    /// Function facts with an incomplete local CFG, including companions.
    /// Operation resource exhaustion returns an error, never a partial result.
    pub partial_functions: usize,
    pub partial_bounds: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StackBound {
    pub root: String,
    pub bytes: u64,
    pub complete: bool,
    /// Function-relative sites survive movement of the function in the ELF.
    pub holes: BTreeSet<Hole>,
    pub assumptions: BTreeSet<String>,
    pub maximum_frame: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub function: String,
    pub offset: u32,
    pub reason: String,
}

impl Measurements {
    pub(crate) fn of(image: &Image, profile: &Profile) -> Result<Self> {
        let file = oer_analysis_elf::Elf::parse(&image.elf)?;
        let memory = profile
            .memory
            .as_ref()
            .ok_or("sizes: chip has no memory map")?;
        // Retained SRAM is outside the placement gate's application SRAM
        // range; take it from the chip's canonical staged layout.
        let retained = match profile.id.as_str() {
            "esp32s31" => oer_esp32s31_layout::LAYOUT.retained,
            "esp32c5" => oer_esp32c5_layout::LAYOUT.retained,
            _ => return Err(format!("sizes: no retained memory layout for {}", profile.id).into()),
        };
        let mut out = Self::default();
        for section in file.sections().filter(|section| section.allocated) {
            let end = section
                .address
                .checked_add(section.size)
                .ok_or("section address overflow")?;
            let kind = if memory.sram.contains_range(section.address, end)
                || retained.contains_range(section.address, end)
            {
                "sram"
            } else if memory
                .psram
                .is_some_and(|r| r.contains_range(section.address, end))
                || memory
                    .runtime_psram
                    .is_some_and(|r| r.contains_range(section.address, end))
            {
                "psram"
            } else if memory.flash_xip.contains_range(section.address, end) {
                "flash"
            } else {
                return Err(format!(
                    "sizes: section {} at {:#x}..{end:#x} is outside the chip memory map",
                    section.name, section.address
                )
                .into());
            };
            let total = out.totals.entry(kind.into()).or_default();
            if section.executable {
                total.code += section.size;
            }
            if matches!(kind, "sram" | "psram") {
                total.ram += section.size;
            }
            if out
                .sections
                .insert(
                    section.name.into(),
                    Section {
                        memory: kind.into(),
                        executable: section.executable,
                        writable: section.writable,
                        address: section.address,
                        bytes: section.size,
                    },
                )
                .is_some()
            {
                return Err(format!("sizes: duplicate section {}", section.name).into());
            }
        }
        let functions: BTreeMap<_, _> = image.functions.iter().map(|f| (f.address, f)).collect();
        for (&address, &bytes) in &image.stack_sizes {
            let function = functions
                .get(&address)
                .ok_or_else(|| format!("sizes: unnamed frame at {address:#x}"))?;
            for name in &function.names {
                out.frames
                    .entry(oer_analysis_elf::demangle(name))
                    .or_default()
                    .push(bytes);
            }
        }
        for frames in out.frames.values_mut() {
            frames.sort_unstable();
        }
        out.types = types::read(&file)?;
        out.resources = Resources {
            work_units: image.usage.work_units,
            peak_memory_bytes: image.usage.memory_bytes,
            work_limit: crate::stack::budget::WORK_UNITS,
            memory_limit_bytes: crate::stack::budget::MEMORY_LIMIT,
            timeout_seconds: crate::stack::budget::TIMEOUT.as_secs(),
            partial_functions: image
                .analysis
                .functions
                .values()
                .filter(|f| !f.complete)
                .count(),
            partial_bounds: 0,
        };
        Ok(out)
    }

    pub(crate) fn task(&mut self, image: &Image, stack: &crate::stack::StackBound) {
        self.allocated_stacks
            .insert(stack.name.into(), stack.linked.0);
        self.bound(image, stack.name, &stack.linked.1, 0);
    }

    pub(crate) fn interrupts(
        &mut self,
        image: &Image,
        stacks: &crate::interrupts::InterruptStacks,
    ) -> Result<()> {
        for hart in &stacks.harts {
            let name = format!("irq hart {}", hart.core);
            let bottom = format!("__runtime_cpu{}_irq_stack_bottom", hart.core);
            let top = format!("__runtime_cpu{}_irq_stack_top", hart.core);
            let storage = image
                .storage(Storage::Range {
                    bottom: &bottom,
                    top: &top,
                })?
                .ok_or_else(|| format!("sizes: missing {name} storage"))?;
            self.allocated_stacks.insert(name.clone(), storage.capacity);
            let mut holes = BTreeSet::new();
            let mut maximum_frame = 0;
            for level in hart.levels.iter().chain([&hart.exception]) {
                let key = format!("{name} level {}", level.level);
                self.bound(image, &key, &level.bound, level.frame);
                let bound = &self.bounds[&key];
                holes.extend(bound.holes.iter().cloned());
                maximum_frame = maximum_frame.max(bound.maximum_frame);
            }
            self.insert_bound(
                name,
                StackBound {
                    root: "nested IRQ levels + exception".into(),
                    bytes: hart.bytes.unwrap_or(hart.partial),
                    complete: hart.bytes.is_some(),
                    holes,
                    assumptions: hart.assumptions().iter().map(ToString::to_string).collect(),
                    maximum_frame,
                },
            );
        }
        Ok(())
    }

    fn bound(&mut self, image: &Image, name: &str, bound: &Bound, entry_frame: u64) {
        let mut holes = BTreeSet::new();
        for &(site, reason) in &bound.unresolved {
            let (function, offset) = match image.analysis.functions.range(..=site).next_back() {
                Some((&start, facts)) if site - start < facts.function.size => (
                    oer_analysis_elf::demangle(&facts.function.label()),
                    site - start,
                ),
                _ => (format!("unmapped:{site:#x}"), 0),
            };
            holes.insert(Hole {
                function,
                offset,
                reason: reason.to_string(),
            });
        }
        let root = image
            .analysis
            .functions
            .get(&bound.root)
            .map(|f| oer_analysis_elf::demangle(&f.function.label()))
            .unwrap_or_else(|| format!("{:#x}", bound.root));
        self.insert_bound(
            name.into(),
            StackBound {
                root,
                bytes: bound.bytes.unwrap_or(bound.partial) + entry_frame,
                complete: bound.bytes.is_some(),
                holes,
                assumptions: bound.assumptions.iter().map(ToString::to_string).collect(),
                maximum_frame: bound
                    .reached
                    .iter()
                    .filter_map(|address| image.analysis.functions.get(address)?.frame)
                    .max()
                    .unwrap_or(0)
                    .max(entry_frame),
            },
        );
    }

    fn insert_bound(&mut self, name: String, bound: StackBound) {
        if !bound.complete {
            self.resources.partial_bounds += 1;
        }
        self.bounds.insert(name, bound);
    }
}
