//! The panic check: the panic sites each root of an ELF reaches, by the
//! crate layer of the function that holds them, compared with the image's
//! per-layer reporting budget.
//!
//! A panic site is a direct call or jump from a function a root reaches
//! (over what is resolved) into a panic entry ([`panic_entry`]), from a
//! function that is not one itself. The roots are those of the stack and
//! interrupt analyses: each interrupt context (hart and level, and each
//! hart's exception), each hart's task entry and each task root in the
//! runtime; the bootstrap root in the bootstrap. A site counts once per ELF
//! however many roots reach it.
//! Compiler `panic_const_async_fn_resumed` calls have a separate row outside
//! the budgets and target of 0: the executor's runtime `DONE` check skips
//! completed tasks, and `.await` does not poll a completed child again.
//!
//! Each layer takes its budget from the image's
//! `[package.metadata.open-radio.panic-budget]`, and so do the sites the
//! `Hardware` interrupt contexts reach together (`hardware`, any layer);
//! what it does not name has 0, the target of every budget. Counts above or
//! below the budget are warnings; no site count fails the build.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use oer_image_stack::Analysis;

use crate::Result;
use oer_image_bundle::CheckedElf;

/// The runtime ELF's panic report.
pub const RUNTIME_REPORT: &str = "runtime-panics.txt";
/// The bootstrap ELF's panic report.
pub const BOOTSTRAP_REPORT: &str = "bootstrap-panics.txt";

const ASYNC_FN_RESUMED: &str = "core::panicking::panic_const::panic_const_async_fn_resumed";
const COMPILER_RESUMED: &str = "compiler-async-fn-resumed";

/// The crate layer a function belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Layer {
    /// Generated register access: `chips/**/pac`, `pac-raw`, `*-pac`,
    /// `*-pac-raw`.
    Pac,
    /// `chips/<chip>/hal`.
    Hal,
    /// Every other crate below `chips/`: drivers, platform, adapters.
    Chips,
    Portable,
    Protocols,
    Firmware,
    /// `core`, `alloc`, `compiler_builtins`.
    Core,
    /// `embassy-*`.
    Embassy,
    /// Every other crate.
    ThirdParty,
    /// Functions whose name names no crate: `#[no_mangle]` and assembly
    /// symbols.
    Unnamed,
}

impl Layer {
    pub const ALL: [Self; 10] = [
        Self::Pac,
        Self::Hal,
        Self::Chips,
        Self::Portable,
        Self::Protocols,
        Self::Firmware,
        Self::Core,
        Self::Embassy,
        Self::ThirdParty,
        Self::Unnamed,
    ];

    /// The layer's key in the budget and the reports.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pac => "pac",
            Self::Hal => "hal",
            Self::Chips => "chips",
            Self::Portable => "portable",
            Self::Protocols => "protocols",
            Self::Firmware => "firmware",
            Self::Core => "core",
            Self::Embassy => "embassy",
            Self::ThirdParty => "third-party",
            Self::Unnamed => "unnamed",
        }
    }

    fn of_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|layer| layer.key() == key)
    }

    /// The layer of a workspace package from its directory, relative to
    /// the workspace root; `None` outside the layered roots.
    pub fn of_directory(directory: &std::path::Path) -> Option<Self> {
        let parts: Vec<&str> = directory.iter().filter_map(|part| part.to_str()).collect();
        let last = *parts.last()?;
        Some(match *parts.first()? {
            "chips" if last == "hal" => Self::Hal,
            "chips"
                if ["pac", "pac-raw"].contains(&last)
                    || last.ends_with("-pac")
                    || last.ends_with("-pac-raw") =>
            {
                Self::Pac
            }
            "chips" => Self::Chips,
            "portable" => Self::Portable,
            "protocols" => Self::Protocols,
            "firmware" => Self::Firmware,
            _ => return None,
        })
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.key())
    }
}

/// The layer of each workspace crate, by its Rust name (`oer_esp32s31_hal`).
#[derive(Clone, Debug, Default)]
pub struct Layers {
    crates: BTreeMap<String, Layer>,
}

impl Layers {
    /// The layers of `packages`: (package name, directory relative to the
    /// workspace root). Packages outside the layered roots are left out.
    pub fn of_packages<'a>(
        packages: impl IntoIterator<Item = (&'a str, &'a std::path::Path)>,
    ) -> Self {
        Self {
            crates: packages
                .into_iter()
                .filter_map(|(name, directory)| {
                    Some((name.replace('-', "_"), Layer::of_directory(directory)?))
                })
                .collect(),
        }
    }

    /// The layer of the demangled function `name`: its crate's, the crate
    /// being the first path segment (of the self type, for `<T as Trait>`).
    pub fn of(&self, name: &str) -> Layer {
        let Some(krate) = crate_of(name) else {
            return Layer::Unnamed;
        };
        if let Some(&layer) = self.crates.get(krate) {
            return layer;
        }
        match krate {
            "core" | "alloc" | "compiler_builtins" => Layer::Core,
            _ if krate.starts_with("embassy_") => Layer::Embassy,
            _ => Layer::ThirdParty,
        }
    }
}

/// The crate a demangled function name starts with.
fn crate_of(name: &str) -> Option<&str> {
    let name = name.trim_start_matches(['<', '&', '[', '(', '*', ' ']);
    let name = name
        .strip_prefix("const ")
        .or_else(|| name.strip_prefix("mut "))
        .or_else(|| name.strip_prefix("dyn "))
        .unwrap_or(name)
        .trim_start_matches(['<', '&', '[', '(']);
    let end = name
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|&end| end > 0 && name[end..].starts_with("::"))?;
    Some(&name[..end])
}

/// Demangled compiler panic entries in the pinned toolchain, the panic
/// handler and the image hook: where a panic starts. Checked on the final
/// machine-code call graph, rather than source names, since optimization
/// outlines or removes helpers.
pub(crate) fn panic_entry(name: &str) -> bool {
    name.starts_with("core::panicking::")
        || name.starts_with("core::option::unwrap_failed")
        || name.starts_with("core::option::expect_failed")
        || name.starts_with("core::result::unwrap_failed")
        || (name.starts_with("core::slice::index::slice_index") && name.contains("fail"))
        || name.starts_with("core::cell::panic_already")
        || name.ends_with("::rust_begin_unwind")
        || name == "oer_platform_panic_hook"
}

/// One panic site: a call or jump at `site` in `caller` into the panic entry
/// `entry`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Site {
    pub caller: u32,
    pub site: u32,
    pub entry: u32,
}

/// The panic sites in the functions of `reached`.
pub fn sites(
    analysis: &Analysis,
    names: &BTreeMap<u32, String>,
    reached: &BTreeSet<u32>,
) -> BTreeSet<Site> {
    let entry = |address: u32| names.get(&address).is_some_and(|name| panic_entry(name));
    let mut sites = BTreeSet::new();
    for &caller in reached {
        if entry(caller) {
            continue;
        }
        let Some(facts) = analysis.functions.get(&caller) else {
            continue;
        };
        for transfer in &facts.transfers {
            if let Some(target) = transfer.target.filter(|&target| entry(target)) {
                sites.insert(Site {
                    caller,
                    site: transfer.site,
                    entry: target,
                });
            }
        }
    }
    sites
}

/// One root of an ELF and the panic sites it reaches.
#[derive(Clone, Debug)]
pub struct Root {
    pub label: String,
    /// A `Hardware` interrupt context: it may reach no panic site.
    pub hardware: bool,
    pub sites: BTreeSet<Site>,
}

impl Root {
    /// The root `label` over the functions it reaches.
    pub fn of(
        label: impl Into<String>,
        hardware: bool,
        analysis: &Analysis,
        names: &BTreeMap<u32, String>,
        reached: &BTreeSet<u32>,
    ) -> Self {
        Self {
            label: label.into(),
            hardware,
            sites: sites(analysis, names, reached),
        }
    }
}

/// The roots of the interrupt analysis: each hart's levels, `Hardware`
/// ones marked, and its exception.
pub(crate) fn interrupt_roots(
    image: &crate::stack::Image,
    stacks: &crate::interrupts::InterruptStacks,
) -> Vec<Root> {
    let mut roots = Vec::new();
    for hart in &stacks.harts {
        for level in &hart.levels {
            roots.push(Root::of(
                format!("hart{}-P{}", hart.core, level.level),
                stacks.hardware_level(hart.core, level.level),
                &image.analysis,
                &image.names,
                &level.bound.reached,
            ));
        }
        roots.push(Root::of(
            format!("hart{}-exception", hart.core),
            false,
            &image.analysis,
            &image.names,
            &hart.exception.bound.reached,
        ));
    }
    roots
}

/// The roots of a stack audit: each hart's task entry and each task root,
/// or the bootstrap root.
pub(crate) fn stack_roots(
    image: &crate::stack::Image,
    audit: &crate::stack::StackAudit,
    policy: &oer_chip_profile::policy::StackPolicy,
) -> Vec<Root> {
    audit
        .roots(policy)
        .into_iter()
        .map(|(label, bound)| Root::of(label, false, &image.analysis, &image.names, &bound.reached))
        .collect()
}

/// The budget of one ELF: by layer, and for the `Hardware` contexts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Limits {
    pub layers: BTreeMap<Layer, usize>,
    /// Distinct sites over all `Hardware` interrupt contexts, any layer.
    pub hardware: usize,
}

/// The image's panic budget of each ELF.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Budget {
    pub runtime: Limits,
    pub bootstrap: Limits,
}

/// The budget key of the `Hardware` interrupt contexts.
pub const HARDWARE: &str = "hardware";

impl Budget {
    /// The budget an image declares under `panic-budget`, a table of
    /// `runtime` and `bootstrap`, each a table of layer keys (and
    /// `hardware`) and site counts:
    /// `{ runtime = { core = 12, hardware = 2 }, bootstrap = { core = 3 } }`.
    /// Nothing declared is a budget of 0 everywhere.
    pub fn parse(declared: Option<&serde_json::Value>) -> Result<Self> {
        let mut budget = Self::default();
        let Some(declared) = declared else {
            return Ok(budget);
        };
        let table = declared
            .as_object()
            .ok_or("`panic-budget` is a table of `runtime` and `bootstrap`")?;
        for (elf, layers) in table {
            let target = match elf.as_str() {
                "runtime" => &mut budget.runtime,
                "bootstrap" => &mut budget.bootstrap,
                other => {
                    return Err(format!(
                        "`panic-budget` names `{other}`; only `runtime` and `bootstrap` exist"
                    )
                    .into());
                }
            };
            let layers = layers
                .as_object()
                .ok_or_else(|| format!("`panic-budget.{elf}` is a table of layers"))?;
            for (key, count) in layers {
                let count = count
                    .as_u64()
                    .and_then(|count| usize::try_from(count).ok())
                    .ok_or_else(|| {
                        format!("`panic-budget.{elf}.{key}` is a site count, not {count}")
                    })?;
                if key == HARDWARE {
                    target.hardware = count;
                    continue;
                }
                let layer = Layer::of_key(key).ok_or_else(|| {
                    format!(
                        "`panic-budget.{elf}` names the unknown layer `{key}`; the keys are {}, \
                         {HARDWARE}",
                        Layer::ALL.map(Layer::key).join(", ")
                    )
                })?;
                target.layers.insert(layer, count);
            }
        }
        Ok(budget)
    }

    fn limits(&self, elf: CheckedElf) -> &Limits {
        match elf {
            CheckedElf::Runtime => &self.runtime,
            CheckedElf::Bootstrap => &self.bootstrap,
        }
    }

    /// The budget of `layer` in `elf`.
    pub fn of(&self, elf: CheckedElf, layer: Layer) -> usize {
        self.limits(elf).layers.get(&layer).copied().unwrap_or(0)
    }

    /// The budget of the `Hardware` contexts in `elf`.
    pub fn hardware(&self, elf: CheckedElf) -> usize {
        self.limits(elf).hardware
    }
}

/// What the panic check of an image reads besides its ELF.
#[derive(Clone, Debug, Default)]
pub struct Contract {
    pub layers: Layers,
    pub budget: Budget,
}

/// The panic sites of one ELF's roots.
#[derive(Debug)]
pub struct Report<'a> {
    pub elf: CheckedElf,
    pub roots: Vec<Root>,
    pub names: &'a BTreeMap<u32, String>,
}

impl Report<'_> {
    fn name(&self, address: u32) -> String {
        self.names
            .get(&address)
            .cloned()
            .unwrap_or_else(|| format!("{address:#010x}"))
    }

    fn layer(&self, contract: &Contract, site: &Site) -> Layer {
        contract.layers.of(&self.name(site.caller))
    }

    /// Classify the resolved callee symbol, never the caller or panic text.
    fn async_fn_resumed(&self, site: &Site) -> bool {
        self.names
            .get(&site.entry)
            .is_some_and(|name| name == ASYNC_FN_RESUMED)
    }

    /// Every distinct site any root reaches.
    fn all_sites(&self) -> BTreeSet<Site> {
        self.roots
            .iter()
            .flat_map(|root| root.sites.iter().copied())
            .collect()
    }

    /// Every distinct budgeted site a `Hardware` context reaches.
    fn hardware_sites(&self) -> BTreeSet<Site> {
        self.roots
            .iter()
            .filter(|root| root.hardware)
            .flat_map(|root| root.sites.iter().copied())
            .filter(|site| !self.async_fn_resumed(site))
            .collect()
    }

    /// The distinct sites of each layer over all roots, excluding compiler
    /// resumed-future checks from the budgets and target of 0.
    pub fn counts(&self, contract: &Contract) -> BTreeMap<Layer, usize> {
        let mut counts = BTreeMap::new();
        for site in self.all_sites() {
            if self.async_fn_resumed(&site) {
                continue;
            }
            *counts.entry(self.layer(contract, &site)).or_insert(0) += 1;
        }
        counts
    }

    /// Report differences from the budget without rejecting any site count.
    pub fn warnings(&self, contract: &Contract) -> Vec<String> {
        let elf = match self.elf {
            CheckedElf::Runtime => "runtime",
            CheckedElf::Bootstrap => "bootstrap",
        };
        let counts = self.counts(contract);
        let mut warnings = Vec::new();
        let mut hold = |count: usize, budget: usize, what: String, key: &str| {
            if count > budget {
                warnings.push(format!(
                    "{elf} reaches {count} panic sites {what}, over its budget of {budget}"
                ));
            } else if count < budget {
                warnings.push(format!(
                    "{elf} reaches {count} panic sites {what}, under its budget of {budget}: \
                     lower `panic-budget.{elf}.{key}` to {count}"
                ));
            }
        };
        for layer in Layer::ALL {
            hold(
                counts.get(&layer).copied().unwrap_or(0),
                contract.budget.of(self.elf, layer),
                format!("in the {layer} layer"),
                layer.key(),
            );
        }
        hold(
            self.hardware_sites().len(),
            contract.budget.hardware(self.elf),
            "from its hardware interrupt contexts".to_owned(),
            HARDWARE,
        );
        warnings
    }

    /// The report: the counts and budget by layer, each root's sites by
    /// layer, compiler resumed-future checks separately, and every site with
    /// the roots that reach it.
    pub fn render(&self, contract: &Contract) -> String {
        let mut out = String::new();
        let counts = self.counts(contract);
        let total: usize = counts.values().sum();
        let _ = writeln!(
            out,
            "panic sites (excluding compiler resumed-future checks, distinct over all roots): {total}"
        );
        let _ = writeln!(out, "layer\tsites\tbudget");
        for layer in Layer::ALL {
            let _ = writeln!(
                out,
                "{layer}\t{}\t{}",
                counts.get(&layer).copied().unwrap_or(0),
                contract.budget.of(self.elf, layer)
            );
        }
        let _ = writeln!(
            out,
            "{HARDWARE}\t{}\t{}",
            self.hardware_sites().len(),
            contract.budget.hardware(self.elf)
        );
        let resumed = self
            .all_sites()
            .iter()
            .filter(|site| self.async_fn_resumed(site))
            .count();
        let _ = writeln!(out, "{COMPILER_RESUMED}\t{resumed}\texcluded");
        for warning in self.warnings(contract) {
            let _ = writeln!(out, "warning: {warning}");
        }
        let _ = writeln!(out, "roots:");
        for root in &self.roots {
            let mut by_layer: BTreeMap<Layer, usize> = BTreeMap::new();
            let resumed = root
                .sites
                .iter()
                .filter(|site| self.async_fn_resumed(site))
                .count();
            for site in &root.sites {
                if self.async_fn_resumed(site) {
                    continue;
                }
                *by_layer.entry(self.layer(contract, site)).or_insert(0) += 1;
            }
            let _ = writeln!(
                out,
                "  {}{}: {}{}; {COMPILER_RESUMED} {resumed}",
                root.label,
                if root.hardware { " (hardware)" } else { "" },
                by_layer.values().sum::<usize>(),
                if by_layer.is_empty() {
                    String::new()
                } else {
                    format!(
                        " ({})",
                        by_layer
                            .iter()
                            .map(|(layer, count)| format!("{layer} {count}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            );
        }
        let _ = writeln!(out, "sites:");
        for site in self.all_sites() {
            let roots: Vec<&str> = self
                .roots
                .iter()
                .filter(|root| root.sites.contains(&site))
                .map(|root| root.label.as_str())
                .collect();
            let _ = writeln!(
                out,
                "  {}\t{} at {:#010x} calls {}\t[{}]",
                if self.async_fn_resumed(&site) {
                    COMPILER_RESUMED.to_owned()
                } else {
                    self.layer(contract, &site).to_string()
                },
                self.name(site.caller),
                site.site,
                self.name(site.entry),
                roots.join(", ")
            );
        }
        out
    }
}

#[cfg(test)]
mod tests;
