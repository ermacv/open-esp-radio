//! The copies of a vendor function the ROM and a pinned library both define,
//! compared by their MMIO behavior (`cargo vendor-check copies`).
//!
//! A fix that lands in a library copy while the ROM copy stays wrong, or the
//! reverse, passes every fingerprint check: each copy is unchanged against
//! its own pin. This report runs Blobray's `mmio-diff` between the chip's
//! pinned ROM ELFs and its pinned libraries inside the register model's MMIO
//! windows, and, for a function a built vendor firmware links, between the
//! ROM and that linked image, where every library address is resolved. A
//! function the analysis sees no difference in is not proven equal: an
//! incomplete analysis on either side may hide one.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use oer_analysis_blobray_application::in_process::{Executable, Limits};
use oer_analysis_blobray_application::library::Assumptions;
use oer_analysis_blobray_domain::{
    ImageRegion, MmioAccess, MmioDiff, MmioFunctionChange, MmioFunctionState, MmioGap,
    MmioOperation, MmioStoredSource, MmioUncertainFunction,
};
use oer_analysis_riscv_model::{
    AnalysisLimited, AnalysisProfile, AnalysisResources, AnalysisUsage, CallAbi, OperationLimits,
    ReadOnlyWindow, WorkingMemory,
};
use serde::Serialize;

use crate::Result;

/// Schema of `target/verification/<chip>/copies.json`.
pub const SCHEMA: u32 = 3;

/// What the comparison of one function's copies shows.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The copies write MMIO differently and both analyses are complete.
    Differs,
    /// Both analyses are complete and both copies write the same words, but
    /// where they differ one side's stored bits are unknown to the analysis:
    /// the difference may be only what the analysis resolves.
    StoredValueUnknown,
    /// A side holds several copies of the name (local functions of several
    /// objects, or one function in several archives): its accesses form one
    /// multiset, so no copy is compared on its own.
    SeveralCopies,
    /// Both analyses are complete and only reads differ: which words are
    /// loaded, or which bits of a loaded word are used. Equivalent code can
    /// test a bit in ways the analysis describes differently (a mask, or a
    /// shift and a sign test), so this is a weaker signal.
    ReadsDiffer,
    /// The copies' accesses differ, but an analysis is incomplete: some of
    /// the difference may be accesses one side does not see.
    DiffersIncomplete,
    /// No access differs, but how many addresses each analysis leaves
    /// unresolved, or whether it is complete, does.
    UnresolvedDiffers,
    /// A copy is not analyzed (blocked): its accesses are unknown.
    Unknown,
    /// The analysis sees no difference; not a proof of equality.
    NoDifferenceSeen,
}

/// One function a ROM and a library both define, for one ROM: a chip with
/// several ROM revisions has an entry per revision.
#[derive(Clone, Debug, Serialize)]
pub struct Function {
    pub name: String,
    /// The ROM copy, as `artifact[]::symbol`.
    pub rom: String,
    /// Its library copies, as `artifact[member]::symbol`.
    pub libraries: Vec<String>,
    /// The comparison the verdict comes from: a linked firmware image by
    /// artifact id, or `archives`.
    pub compared: String,
    pub verdict: Verdict,
    /// Whether each side's analysis is complete: ROM, library.
    pub complete: [bool; 2],
    /// Accesses only the ROM copy makes.
    pub rom_only: Vec<MmioAccess>,
    /// Accesses only the library copy makes.
    pub library_only: Vec<MmioAccess>,
    /// The copies our facts cite (`chips/<chip>/vendor/facts/provenance.toml`).
    pub cited: Vec<String>,
}

/// The report of one chip.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: u32,
    pub analysis_resources: Vec<OperationResources>,
    pub chip: String,
    /// The pinned ROM ELFs, libraries and built vendor images compared, by
    /// artifact id.
    pub roms: Vec<String>,
    pub libraries: Vec<String>,
    pub images: Vec<String>,
    /// Pinned vendor firmware not built in this checkout: no linked
    /// comparison of its functions.
    pub unbuilt_images: Vec<String>,
    pub windows: Vec<ImageRegion>,
    pub functions: Vec<Function>,
}

/// Resources of one operation; repeated ROM analyses remain separate.
#[derive(Clone, Debug, Serialize)]
pub struct OperationResources {
    pub operation: &'static str,
    pub inputs: Vec<String>,
    pub resources: AnalysisResources,
}

/// One input: its artifact id and bytes.
pub struct Input {
    pub id: String,
    pub bytes: Vec<u8>,
}

/// The copies each function name has in `inputs`, as
/// `artifact[member]::symbol`.
fn copies(inputs: &[Input]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for input in inputs {
        let (functions, _) =
            oer_vendor_provenance::fingerprint::functions_and_symbols(&input.bytes)?;
        for function in functions {
            let copy = format!("{}[{}]::{}", input.id, function.member, function.name);
            by_name.entry(function.name).or_default().push(copy);
        }
    }
    Ok(by_name)
}

/// One comparison's reported functions, by name, and the code it leaves
/// unanalyzed.
struct Compared {
    changes: BTreeMap<String, MmioFunctionChange>,
    uncertain: BTreeMap<String, MmioUncertainFunction>,
    /// `(input, member)` of each object a side leaves unanalyzed, the member
    /// `None` for a whole input: old, new.
    gaps: [BTreeSet<(String, Option<String>)>; 2],
}

impl Compared {
    /// Whether side `side` leaves the copy `copy` (`artifact[member]::name`)
    /// unanalyzed.
    fn unanalyzed(&self, side: usize, copy: &str) -> bool {
        let Some((artifact, rest)) = copy.split_once('[') else {
            return false;
        };
        let member = rest.split_once(']').map_or("", |(member, _)| member);
        self.gaps[side].contains(&(artifact.to_owned(), None))
            || self.gaps[side].contains(&(artifact.to_owned(), Some(member.to_owned())))
    }
}

/// The member name of each object of `input`, by object id.
fn members(
    input: &Input,
    budget: &Budget,
    resources: &std::cell::RefCell<Vec<OperationResources>>,
) -> Result<BTreeMap<oer_analysis_riscv_model::ObjectId, String>> {
    let memory = budget.memory()?;
    let mut control = budget.control();
    let inventory = oer_analysis_blobray_application::captured::inventory(
        &Executable::new(input.bytes.clone()),
        &memory,
        &mut control,
    )?;
    resources.borrow_mut().push(OperationResources {
        operation: "inventory",
        inputs: vec![input.id.clone()],
        resources: budget.resources(&memory, &control, AnalysisLimited::default())?,
    });
    Ok(inventory
        .objects
        .into_iter()
        .map(|o| {
            (
                o.id,
                String::from_utf8_lossy(&o.name.unwrap_or_default()).into_owned(),
            )
        })
        .collect())
}

/// The verdict of one function from the comparison that lists it, or none.
fn verdict(
    change: Option<&MmioFunctionChange>,
    uncertain: Option<&MmioUncertainFunction>,
) -> (Verdict, [bool; 2]) {
    let Some(change) = change else {
        // Unlisted: no difference seen, which an incomplete or blocked
        // analysis does not show absent.
        return match uncertain {
            Some(u) if u.state.contains(&MmioFunctionState::Blocked) => {
                (Verdict::Unknown, u.complete)
            }
            Some(u) => (Verdict::NoDifferenceSeen, u.complete),
            None => (Verdict::NoDifferenceSeen, [true, true]),
        };
    };
    let analyzed = change.state == [MmioFunctionState::Analyzed; 2];
    if !analyzed {
        return (Verdict::Unknown, change.complete);
    }
    let complete = change.complete == [true, true];
    let verdict = if change.removed.is_empty() && change.added.is_empty() {
        Verdict::UnresolvedDiffers
    } else if !complete {
        Verdict::DiffersIncomplete
    } else {
        writes(&change.removed, &change.added)
    };
    (verdict, change.complete)
}

/// How the differing accesses of two complete analyses differ: a store one
/// side lacks, or whose known bits differ, is a write difference; a store
/// the other side makes to the same word with unknown bits is only a value
/// the analysis does not resolve; loads carry their read-use masks.
fn writes(removed: &[MmioAccess], added: &[MmioAccess]) -> Verdict {
    let store = |access: &MmioAccess| access.operation == MmioOperation::Store;
    let unknown = |access: &MmioAccess| {
        access
            .stored
            .iter()
            .any(|run| run.source == MmioStoredSource::Unknown)
    };
    let mut weak = false;
    for (side, other) in [(removed, added), (added, removed)] {
        for access in side.iter().filter(|a| store(a)) {
            let same_word: Vec<_> = other
                .iter()
                .filter(|o| store(o) && o.location == access.location)
                .collect();
            if !same_word.is_empty() && (unknown(access) || same_word.iter().any(|o| unknown(o))) {
                weak = true;
            } else {
                return Verdict::Differs;
            }
        }
    }
    if weak {
        Verdict::StoredValueUnknown
    } else {
        Verdict::ReadsDiffer
    }
}

/// Compare every function a ROM of `roms` and `libraries` both define,
/// inside `windows`, each ROM on its own; a function an image of `images`
/// defines is compared against that linked image (with the chip's
/// `read_only` flash windows), others against the archives. `cited` names
/// the copies our facts cite, by function name. Each comparison runs under
/// its own `budget`.
#[allow(clippy::too_many_arguments)]
pub fn compare(
    chip: &str,
    roms: &[Input],
    libraries: &[Input],
    images: &[Input],
    windows: &[ImageRegion],
    read_only: &[ReadOnlyWindow],
    cited: &BTreeMap<String, Vec<String>>,
    budget: &Budget,
) -> Result<Report> {
    let resources = std::cell::RefCell::new(Vec::new());
    let library_copies = copies(libraries)?;
    let decoder = oer_analysis_riscv_lift::RiscvDecoder;
    let named_gaps =
        |inputs: &[&Input], gaps: Vec<MmioGap>| -> Result<BTreeSet<(String, Option<String>)>> {
            let mut named = BTreeSet::new();
            for gap in gaps {
                let input = inputs
                    .get(gap.input as usize)
                    .ok_or("a gap names an input the comparison was not given")?;
                let member = match gap.object {
                    Some(object) => Some(
                        members(input, budget, &resources)?
                            .remove(&object)
                            .unwrap_or_default(),
                    ),
                    None => None,
                };
                named.insert((input.id.clone(), member));
            }
            Ok(named)
        };
    let diff = |old: &Input, new: &[&Input], read_only: &[ReadOnlyWindow]| -> Result<Compared> {
        let memory = budget.memory()?;
        let mut control = budget.control();
        let (diff, limited): (MmioDiff, _) =
            oer_analysis_blobray_application::mmio_diff::mmio_diff(
                &[Executable::new(old.bytes.clone())],
                &new.iter()
                    .map(|i| Executable::new(i.bytes.clone()))
                    .collect::<Vec<_>>(),
                Assumptions {
                    functions: &[],
                    symbolic_stores: false,
                    pointer_chains: false,
                    abi: Some(CallAbi::RiscvInteger),
                    call_summaries: false,
                    signatures: None,
                    call_results: &[],
                    read_only,
                },
                windows,
                &decoder,
                &memory,
                &mut control,
            )?;
        resources.borrow_mut().push(OperationResources {
            operation: "mmio-diff",
            inputs: std::iter::once(old)
                .chain(new.iter().copied())
                .map(|i| i.id.clone())
                .collect(),
            resources: budget.resources(&memory, &control, limited)?,
        });
        let [old_gaps, new_gaps] = diff.gaps;
        Ok(Compared {
            changes: diff
                .changes
                .into_iter()
                .map(|c| (c.function.clone(), c))
                .collect(),
            uncertain: diff
                .uncertain
                .into_iter()
                .map(|u| (u.function.clone(), u))
                .collect(),
            gaps: [named_gaps(&[old], old_gaps)?, named_gaps(new, new_gaps)?],
        })
    };
    let library_inputs: Vec<&Input> = libraries.iter().collect();
    // The copies each image holds of each name.
    let mut image_copies = Vec::new();
    for image in images {
        let (functions, _) =
            oer_vendor_provenance::fingerprint::functions_and_symbols(&image.bytes)?;
        let mut names: BTreeMap<String, usize> = BTreeMap::new();
        for function in functions {
            *names.entry(function.name).or_default() += 1;
        }
        image_copies.push(names);
    }
    let mut functions = Vec::new();
    // Each ROM is compared on its own: one build's copies of a name form one
    // multiset of accesses, so two ROM revisions compared at once would
    // double every access.
    for rom in roms {
        let archives = diff(rom, &library_inputs, &[])?;
        let mut linked = Vec::new();
        for (image, names) in images.iter().zip(&image_copies) {
            linked.push((image.id.clone(), names, diff(rom, &[image], read_only)?));
        }
        for (name, rom_copies) in copies(std::slice::from_ref(rom))? {
            let Some(library) = library_copies.get(&name) else {
                continue;
            };
            let (compared, against, other_copies) = match linked
                .iter()
                .find(|(_, names, _)| names.contains_key(&name))
            {
                Some((id, names, against)) => {
                    let image = vec![format!("{id}[]::{name}"); names[&name]];
                    (id.clone(), against, image)
                }
                None => ("archives".to_owned(), &archives, library.clone()),
            };
            let change = against.changes.get(&name);
            let uncertain = against.uncertain.get(&name);
            let (mut verdict, mut complete) = verdict(change, uncertain);
            // A copy in code the analysis skips leaves its side incomplete,
            // whatever the comparison of the other copies shows.
            let unanalyzed = [
                rom_copies.iter().any(|c| against.unanalyzed(0, c)),
                other_copies.iter().any(|c| against.unanalyzed(1, c)),
            ];
            for (complete, unanalyzed) in complete.iter_mut().zip(unanalyzed) {
                *complete &= !unanalyzed;
            }
            if rom_copies.len() > 1 || other_copies.len() > 1 {
                verdict = Verdict::SeveralCopies;
            } else if change.is_none() && uncertain.is_none() && unanalyzed.contains(&true) {
                // Unlisted: matched only where both sides analyzed it.
                verdict = Verdict::Unknown;
            }
            functions.push(Function {
                rom: rom_copies.join(" "),
                libraries: library.clone(),
                compared,
                verdict,
                complete,
                rom_only: change.map(|c| c.removed.clone()).unwrap_or_default(),
                library_only: change.map(|c| c.added.clone()).unwrap_or_default(),
                cited: cited.get(&name).cloned().unwrap_or_default(),
                name,
            });
        }
    }
    functions.sort_by(|a, b| (a.verdict, &a.name, &a.rom).cmp(&(b.verdict, &b.name, &b.rom)));
    Ok(Report {
        schema: SCHEMA,
        analysis_resources: resources.into_inner(),
        chip: chip.to_owned(),
        roms: roms.iter().map(|i| i.id.clone()).collect(),
        libraries: libraries.iter().map(|i| i.id.clone()).collect(),
        images: images.iter().map(|i| i.id.clone()).collect(),
        unbuilt_images: vec![],
        windows: windows.to_vec(),
        functions,
    })
}

/// The chip's pinned inputs, MMIO windows and cited copies, compared and
/// written to `target/verification/<chip>/copies.json`; the summary and the
/// functions whose copies differ are printed.
pub fn run(root: &Path, chip: &str, budget: &Budget) -> Result<()> {
    let mut roms = Vec::new();
    let mut libraries = Vec::new();
    let mut images = Vec::new();
    for artifact in oer_tool_pins::artifacts::pinned(root, chip)? {
        let bytes = std::fs::read(&artifact.path)
            .map_err(|e| format!("{}: {e}", artifact.path.display()))?;
        if !oer_analysis_elf::is_binary(&bytes) {
            continue;
        }
        let input = Input {
            id: artifact.id,
            bytes,
        };
        // A linked executable is a firmware image or the ROM; an archive,
        // fetched or built with a firmware, is a library.
        match (
            oer_analysis_elf::Elf::executable(&input.bytes).is_ok(),
            artifact.firmware,
        ) {
            (true, true) => images.push(input),
            (true, false) => roms.push(input),
            (false, _) => libraries.push(input),
        }
    }
    let unbuilt_images = oer_tool_pins::artifacts::unbuilt_firmware(root, chip)?;
    // The MMIO windows: every instance of the chip root.
    let model = oer_registers_model::Chip::load(root, chip)?.model;
    let windows = model
        .instances()
        .iter()
        .map(|instance| -> Result<ImageRegion> {
            Ok(ImageRegion {
                start: u32::try_from(instance.base).map_err(|_| "MMIO instance above 4 GiB")?,
                length: instance.extent.size,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // The flash a linked image's rodata sits in is read-only at run time,
    // whatever its section flags say.
    let read_only = match oer_chip_profile::Profile::load(root, chip)?.facts.memory {
        Some(memory) => vec![ReadOnlyWindow {
            start: memory.flash_xip.origin(),
            length: u64::from(memory.flash_xip.length()),
        }],
        None if images.is_empty() => vec![],
        None => {
            return Err(format!(
                "{chip}'s chip profile has no [memory] flash window, which the comparison \
                 with its linked vendor images needs"
            )
            .into());
        }
    };
    let mut cited: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in oer_tool_pins::provenance::registered(root, chip)? {
        cited.entry(entry.symbol.clone()).or_default().push(format!(
            "{}[{}]::{}",
            entry.artifact, entry.member, entry.symbol
        ));
    }
    let mut report = compare(
        chip, &roms, &libraries, &images, &windows, &read_only, &cited, budget,
    )?;
    report.unbuilt_images = unbuilt_images;
    let directory = root.join(format!("target/verification/{chip}"));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("copies.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    print!("{}", human(&report));
    println!("report: {}", path.display());
    Ok(())
}

/// The summary and every function whose copies differ.
pub fn human(report: &Report) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let mut counts: BTreeMap<Verdict, usize> = BTreeMap::new();
    for function in &report.functions {
        *counts.entry(function.verdict).or_default() += 1;
    }
    let _ = writeln!(
        out,
        "{}: {} functions with a ROM and a library copy (ROM {}; images {}{})",
        report.chip,
        report.functions.len(),
        report.roms.join(", "),
        if report.images.is_empty() {
            "none".to_owned()
        } else {
            report.images.join(", ")
        },
        if report.unbuilt_images.is_empty() {
            String::new()
        } else {
            format!("; unbuilt {}", report.unbuilt_images.join(", "))
        },
    );
    for (verdict, count) in &counts {
        let _ = writeln!(out, "  {count:5} {verdict:?}");
    }
    for function in &report.functions {
        if !matches!(
            function.verdict,
            Verdict::Differs | Verdict::DiffersIncomplete
        ) {
            continue;
        }
        let _ = writeln!(
            out,
            "{} {:?} {} via {}{}",
            function.name,
            function.verdict,
            function.rom,
            function.compared,
            if function.cited.is_empty() {
                String::new()
            } else {
                format!(", cited {}", function.cited.join(" "))
            },
        );
        for (side, accesses) in [("ROM", &function.rom_only), ("lib", &function.library_only)] {
            for access in accesses.iter().take(4) {
                let _ = writeln!(
                    out,
                    "  {side} only: {}",
                    serde_json::to_string(access).unwrap_or_default()
                );
            }
        }
    }
    out
}

/// The Blobray budget of the comparisons.
#[derive(Clone, Copy, Debug, clap::Args)]
pub struct Budget {
    /// Wall-clock deadline of each comparison.
    #[arg(long, default_value_t = AnalysisProfile::VendorCopies.limits().timeout_ms / 1000)]
    pub timeout_secs: u64,
    /// Algorithm working memory of each comparison.
    #[arg(long, default_value_t = AnalysisProfile::VendorCopies.limits().working_memory_bytes >> 20)]
    pub working_memory_mib: u64,
    /// Work-unit budget of each comparison.
    #[arg(long, default_value_t = AnalysisProfile::VendorCopies.limits().work_units)]
    pub max_work_units: u64,
}

impl Budget {
    fn resources(
        &self,
        memory: &WorkingMemory,
        control: &Limits,
        limited: AnalysisLimited,
    ) -> Result<AnalysisResources> {
        Ok(AnalysisResources::new(
            AnalysisProfile::VendorCopies,
            OperationLimits {
                work_units: self.max_work_units,
                working_memory_bytes: memory.observation().limit_bytes,
                timeout_ms: self
                    .timeout_secs
                    .checked_mul(1000)
                    .ok_or("timeout milliseconds overflow")?,
            },
            AnalysisUsage {
                work_units: control.used(),
                peak_admitted_bytes: memory.peak(),
            },
            limited,
        ))
    }
    fn memory(&self) -> Result<WorkingMemory> {
        Ok(WorkingMemory::new(self.working_memory_mib << 20)?)
    }
    fn control(&self) -> Limits {
        Limits::new(
            self.max_work_units,
            std::time::Duration::from_secs(self.timeout_secs),
        )
    }
}

#[cfg(test)]
mod tests;
