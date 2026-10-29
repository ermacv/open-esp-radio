//! Placement selected by the interrupt analysis after the first link.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::interrupts::{InterruptPlacement, interrupt_stacks_of};
use crate::stack::{Image, StackAudit};
use oer_chip_profile::policy::StackPolicy;
use oer_os_durable::inputs::Inputs;

mod fingerprint;
pub use fingerprint::Fingerprint;

/// An analysis specification; only links using the runtime script consume it.
pub const SPEC_ENV: &str = "OER_IMAGE_INTERRUPTS";
pub const SPEC_FILE: &str = "image-linker.toml";
pub const FRAGMENT: &str = "strict-text.x";
pub const PLACEMENT: &str = "strict-placement.toml";

/// The same chip, stack policy and pinned ROM/summaries as the image check.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub root: PathBuf,
    pub chip: String,
    pub stack_policy: PathBuf,
    /// Diagnostic/probe images retain the contract maximum on every hart.
    #[serde(default)]
    pub diagnostic: bool,
    /// The image declares its per-hart task stacks proven: the link sizes it from
    /// the bound of the first ELF.
    #[serde(default)]
    pub task_stack_proven: bool,
}

impl Spec {
    pub fn read(path: &Path, inputs: &mut Inputs) -> Result<Self> {
        Ok(toml::from_str(&inputs.read_to_string(path)?)?)
    }

    fn image(
        &self,
        elf: &Path,
        inputs: &mut Inputs,
    ) -> Result<Option<(Image, oer_chip_profile::Profile, StackPolicy)>> {
        let profile = oer_chip_profile::Profile::load_with_inputs(&self.root, &self.chip, inputs)?;
        if profile.facts.memory.is_none() || profile.facts.interrupts.is_none() {
            return Ok(None);
        }
        let policy =
            oer_chip_profile::policy::StackPolicy::load_with_inputs(&self.stack_policy, inputs)?;
        let profile = policy.runtime_profile(&profile)?;
        let image = Image::read_with_inputs(&self.root, &profile, elf, inputs)?;
        Ok(Some((image, profile, policy)))
    }
}

/// An executable input section and the symbols its object actually defines.
#[derive(Clone, Debug)]
pub(super) struct InputSection {
    /// File, or `archive:member`, in LLD's input-file selector syntax.
    pub file: String,
    pub index: usize,
    pub name: String,
    pub size: u64,
    pub symbols: Vec<String>,
}

#[derive(Debug)]
struct Requested {
    name: String,
    symbols: Vec<String>,
    size: u64,
}

/// One strict function moved with its whole defining input section.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedFunction {
    pub name: String,
    pub symbol: String,
    pub file: String,
    pub section: String,
    pub section_index: usize,
    pub size: u64,
    pub section_bytes: u64,
    pub other_symbols: Vec<String>,
}

/// The linker selection, preserved beside the fragment in the image bundle.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default)]
    pub functions: Vec<PlacedFunction>,
    #[serde(default)]
    pub irq_stacks: Vec<super::irq_stacks::Allocation>,
    /// The per-hart task stacks of an image that declares it proven.
    #[serde(default)]
    pub task_stacks: Vec<super::task_stack::Allocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
}

impl Placement {
    /// No selection metadata means this image did not use automatic placement.
    pub fn read(directory: &Path) -> Result<Self> {
        match std::fs::read_to_string(directory.join(PLACEMENT)) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn select(requested: &[Requested], sections: &[InputSection], free: u64) -> Result<Self> {
        let mut functions = Vec::new();
        let mut moved = BTreeSet::new();
        let mut bytes = 0_u64;
        for function in requested {
            for symbol in &function.symbols {
                script_name(symbol, &function.name, false)?;
            }
            let candidates: Vec<_> = sections
                .iter()
                .filter(|section| {
                    function
                        .symbols
                        .iter()
                        .any(|symbol| section.symbols.contains(symbol))
                })
                .collect();
            let [section] = candidates.as_slice() else {
                return Err(format!(
                    "strict function {} has {} defining input sections; expected exactly one",
                    function.name,
                    candidates.len()
                )
                .into());
            };
            script_name(&section.name, &function.name, false)?;
            script_name(&section.file, &function.name, true)?;
            let symbol = function
                .symbols
                .iter()
                .find(|symbol| section.symbols.contains(symbol))
                .ok_or("selected section has no defining symbol")?;
            // ELF can contain several input sections with the same name.
            // LLD's file/section selector moves all of them, so account for
            // that entire group, including every co-resident symbol.
            let group: Vec<_> = sections
                .iter()
                .filter(|candidate| {
                    candidate.file == section.file && candidate.name == section.name
                })
                .collect();
            let section_bytes = group.iter().try_fold(0_u64, |total, section| {
                total
                    .checked_add(section.size)
                    .ok_or("strict input section size overflow")
            })?;
            if moved.insert((&section.file, &section.name)) {
                bytes = bytes
                    .checked_add(section_bytes)
                    .ok_or("strict input section size overflow")?;
            }
            functions.push(PlacedFunction {
                name: function.name.clone(),
                symbol: symbol.clone(),
                file: section.file.clone(),
                section: section.name.clone(),
                section_index: section.index,
                size: function.size,
                section_bytes,
                other_symbols: group
                    .iter()
                    .flat_map(|section| &section.symbols)
                    .filter(|name| *name != symbol)
                    .cloned()
                    .collect(),
            });
        }
        if bytes > free {
            return Err(format!("strict input sections need {bytes} bytes; only {free} bytes of SRAM are free (short by {} bytes)", bytes - free).into());
        }
        Ok(Self {
            functions,
            irq_stacks: Vec::new(),
            task_stacks: Vec::new(),
            fingerprint: None,
        })
    }

    /// Empty when the first link needs no relocation. Exact file and section
    /// selectors cannot collect a similarly named section from another object.
    pub fn fragment(&self) -> String {
        let sections: BTreeSet<_> = self
            .functions
            .iter()
            .map(|function| (&function.file, &function.section))
            .collect();
        let mut text = String::new();
        for (file, section) in sections {
            let _ = writeln!(text, "\"{file}\"(\"{section}\");");
        }
        text
    }

    pub fn summary(&self) -> String {
        format!(
            "strict code placed by the image linker: {} functions, {} bytes\n",
            self.functions.len(),
            self.functions
                .iter()
                .map(|function| function.size)
                .sum::<u64>()
        )
    }

    pub fn render(&self) -> String {
        let mut text = self.summary();
        text.push_str("\nStrict code placed by the image linker (whole input sections)\nname\tinput file\tinput section\tsize\tsection bytes before relaxation\tother symbols of the section\n");
        for function in &self.functions {
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}",
                function.name,
                function.file,
                function.section,
                function.size,
                function.section_bytes,
                function.other_symbols.join(", ")
            );
        }
        text
    }

    /// Relaxation after relocation can change the final function sizes.
    fn final_sizes(&mut self, output: &Path) -> Result<()> {
        let bytes = std::fs::read(output)?;
        let elf = oer_analysis_elf::Elf::parse(&bytes)?;
        let functions = elf.functions()?;
        for function in &mut self.functions {
            function.size = u64::from(
                functions
                    .iter()
                    .find(|candidate| candidate.names.contains(&function.symbol))
                    .ok_or_else(|| {
                        format!(
                            "relocated strict function {} is absent from the second ELF",
                            function.name
                        )
                    })?
                    .size,
            );
        }
        Ok(())
    }
}

fn script_name(name: &str, function: &str, file: bool) -> Result<()> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || b"_.$-".contains(&byte)
                || (file && b"/ :".contains(&byte))
        })
    {
        return Err(format!(
            "strict function {function}: `{name}` requires linker-script escaping"
        )
        .into());
    }
    Ok(())
}

/// Analyze the first ELF using the final check's implementation. A partial
/// stack is not a failed analysis and its known strict subset is still placed.
pub(super) fn analyze(
    spec: &Spec,
    output: &Path,
    sections: &[InputSection],
    inputs: &mut Inputs,
) -> Result<Placement> {
    let Some((image, profile, policy)) = spec.image(output, inputs)? else {
        if spec.task_stack_proven {
            return Err("the image declares its task stack proven, but its chip has no memory map and interrupt contract".into());
        }
        return Ok(Placement::default());
    };
    let stacks = interrupt_stacks_of(&image)?;
    let placement = InterruptPlacement::of(&image, &stacks, &profile)?;
    let requested = stacks
        .code_placement
        .strict_cached()
        .map(|(address, size)| {
            let function = image
                .functions
                .iter()
                .find(|function| function.address == address)
                .ok_or_else(|| {
                    format!("strict cached function at {address:#x} has no image symbol")
                })?;
            Ok(Requested {
                name: image
                    .names
                    .get(&address)
                    .cloned()
                    .unwrap_or_else(|| format!("{address:#x}")),
                symbols: function.names.clone(),
                size: u64::from(size),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let irq_stacks = super::irq_stacks::select(&stacks, spec.diagnostic)?;
    let released = irq_stacks.iter().try_fold(0_u64, |sum, stack| -> Result<u64> {
        let previous = stacks.allocated_bytes(stack.hart);
        let released = previous.checked_sub(stack.bytes).ok_or_else(|| {
            format!("hart {}'s first-pass IRQ allocation {previous} is smaller than the selected {} bytes", stack.hart, stack.bytes)
        })?;
        Ok(sum + u64::from(released))
    })?;
    let mut placement = Placement::select(&requested, sections, placement.free_sram() + released)?;
    placement.irq_stacks = irq_stacks;
    if spec.task_stack_proven {
        let audit = StackAudit::of(&image, &policy, &policy.runtime_task_stacks())?;
        placement.task_stacks = super::task_stack::select(&audit, &policy)?;
    }
    Ok(placement)
}

/// The second link is final: verify its bounds and linked allocations before fingerprinting.
pub(super) fn verify(
    spec: &Spec,
    output: &Path,
    placement: &Placement,
    inputs: &mut Inputs,
) -> Result<()> {
    let Some((image, _, policy)) = spec.image(output, inputs)? else {
        return Ok(());
    };
    super::irq_stacks::verify(&interrupt_stacks_of(&image)?, &placement.irq_stacks)?;
    if !placement.task_stacks.is_empty() {
        let audit = StackAudit::of(&image, &policy, &policy.runtime_task_stacks())?;
        super::task_stack::verify(&audit, &placement.task_stacks)?;
    }
    Ok(())
}

/// Finish the selection record after at most one additional link.
pub(super) fn save(
    mut placement: Placement,
    directory: &Path,
    output: &Path,
    inputs: Inputs,
) -> Result<()> {
    if !placement.functions.is_empty() {
        placement.final_sizes(output)?;
    }
    placement.fingerprint = Some(Fingerprint::capture(
        output,
        &std::env::current_exe()?,
        inputs,
    )?);
    std::fs::write(directory.join(PLACEMENT), toml::to_string(&placement)?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
