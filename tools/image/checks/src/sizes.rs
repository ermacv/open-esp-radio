//! Image size measurements. These observe the checks' existing analysis;
//! they neither run an analysis nor decide a check's verdict.
use crate::{Result, stack::Image};
use oer_analysis_riscv_stack::Bound;
use oer_chip_profile::{Profile, policy::Storage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod padding;
mod types;

/// Aggregate and array layouts smaller than this are omitted from the inventory.
pub const TYPE_MIN_BYTES: u64 = 256;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measurements {
    pub sections: BTreeMap<String, Section>,
    /// Raw executable, read-only and filler bytes, and owned RAM, by memory kind.
    /// Embedded payload deduplication is recorded by the bundle summary.
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
    pub class: SectionClass,
    pub filler: Vec<Filler>,
    pub nobits: bool,
    pub executable: bool,
    pub writable: bool,
    pub address: u64,
    pub bytes: u64,
}

/// An input-free range defined by a canonical linker alignment contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Filler {
    pub address: u64,
    pub bytes: u64,
    pub contract: String,
}

/// Contents and storage are independent: executable/read-only SRAM is also RAM.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionClass {
    Code,
    Writable,
    ReadOnly,
    Filler,
}

impl Section {
    pub fn is_ram(&self) -> bool {
        self.class != SectionClass::Filler
            && (self.memory == "sram" || (self.memory == "psram" && self.writable))
    }

    /// SRAM only: the memory whose growth stops a pull request.
    pub fn is_sram(&self) -> bool {
        self.class != SectionClass::Filler && self.memory == "sram"
    }

    pub fn image_content_bytes(&self) -> u64 {
        if self.is_image_content() {
            self.bytes - self.filler.iter().map(|r| r.bytes).sum::<u64>()
        } else {
            0
        }
    }

    pub fn is_image_content(&self) -> bool {
        !self.nobits && matches!(self.class, SectionClass::Code | SectionClass::ReadOnly)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    pub code: u64,
    pub read_only: u64,
    pub filler: u64,
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
    /// Offsets retain separate machine sites; comparison uses source identity
    /// and multiplicity rather than these layout-dependent offsets.
    pub holes: BTreeSet<Hole>,
    pub assumptions: BTreeSet<String>,
    pub maximum_frame: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub function: String,
    pub offset: u32,
    pub inline_chain: Vec<String>,
    pub reason: String,
}

impl Measurements {
    /// Reobserve an existing ELF with the normal, unchanged analysis budget.
    /// No compilation, linking or check reports are produced.
    pub fn remeasure(
        root: &std::path::Path,
        profile: &Profile,
        policy: &oer_chip_profile::policy::StackPolicy,
        elf: &std::path::Path,
        bootstrap: bool,
    ) -> Result<Self> {
        let runtime;
        let profile = if bootstrap {
            profile
        } else {
            runtime = policy.runtime_profile(profile)?;
            &runtime
        };
        let image = Image::read(root, profile, elf)?;
        let mut measured = Self::of(&image, profile)?;
        let tasks = if bootstrap {
            policy
                .stacks()?
                .bootstrap_stack
                .map(|s| vec![("bootstrap stack", s)])
                .unwrap_or_default()
        } else {
            policy.runtime_task_stacks()
        };
        let audit = crate::stack::StackAudit::of(&image, policy, &tasks)?;
        for stack in &audit.stacks {
            measured.task(&image, stack)?;
        }
        if !bootstrap && profile.facts.interrupts.is_some() {
            measured.interrupts(&image, &crate::interrupts::interrupt_stacks_of(&image)?)?;
        }
        Ok(measured)
    }

    pub(crate) fn of(image: &Image, profile: &Profile) -> Result<Self> {
        let file = oer_analysis_elf::Elf::parse(&image.elf)?;
        let memory = profile
            .facts
            .memory
            .as_ref()
            .ok_or("sizes: chip has no memory map")?;
        // Retained SRAM is outside the placement gate's application SRAM
        // range; the chip facts name it.
        let retained = memory
            .retained
            .ok_or("sizes: chip has no retained memory")?;
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
            let filler = padding::ranges(&file, &section);
            let measured = Section {
                memory: kind.into(),
                class: if filler
                    .iter()
                    .any(|r| r.address == section.address && r.bytes == section.size)
                {
                    SectionClass::Filler
                } else if section.executable {
                    SectionClass::Code
                } else if section.writable {
                    SectionClass::Writable
                } else {
                    SectionClass::ReadOnly
                },
                filler,
                nobits: section.nobits,
                executable: section.executable,
                writable: section.writable,
                address: section.address,
                bytes: section.size,
            };
            let total = out.totals.entry(kind.into()).or_default();
            match measured.class {
                SectionClass::Code if !measured.nobits => {
                    total.code += measured.image_content_bytes()
                }
                SectionClass::ReadOnly if !measured.nobits => {
                    total.read_only += measured.image_content_bytes()
                }
                SectionClass::Filler => {}
                _ => {}
            }
            total.filler += measured.filler.iter().map(|r| r.bytes).sum::<u64>();
            if measured.is_ram() {
                total.ram += measured.bytes;
            }
            if out.sections.insert(section.name.into(), measured).is_some() {
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
            work_units: image.resources.usage.work_units,
            peak_memory_bytes: image.resources.usage.peak_admitted_bytes,
            work_limit: crate::stack::budget::LIMITS.work_units,
            memory_limit_bytes: crate::stack::budget::LIMITS.working_memory_bytes,
            timeout_seconds: crate::stack::budget::LIMITS.timeout_ms / 1000,
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

    pub(crate) fn task(&mut self, image: &Image, stack: &crate::stack::StackBound) -> Result<()> {
        self.allocated_stacks
            .insert(stack.name.into(), stack.linked.0);
        self.bound(image, stack.name, &stack.linked.1, 0)
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
                self.bound(image, &key, &level.bound, level.frame)?;
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

    fn bound(&mut self, image: &Image, name: &str, bound: &Bound, entry_frame: u64) -> Result<()> {
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
                inline_chain: image.dwarf.inline_chain(site)?,
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
        Ok(())
    }

    fn insert_bound(&mut self, name: String, bound: StackBound) {
        if !bound.complete {
            self.resources.partial_bounds += 1;
        }
        self.bounds.insert(name, bound);
    }
}
