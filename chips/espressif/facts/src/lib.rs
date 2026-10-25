//! The facts of the Espressif chips, in one layered model.
//!
//! Each fact has one layer (README): the ISA of the harts
//! (`chips/espressif/platform/isa/`), the family
//! (`chips/espressif/platform/family.toml`), the chip
//! (`chips/<id>/platform/chip.toml`), the module that carries it
//! (`chips/<id>/platform/modules/`) and the board that carries the module
//! (`chips/<id>/platform/boards/`). A layer `extends` the layers below it
//! ([`resolve`]); [`load`] resolves a chip from its board and
//! [`Facts::from_table`] reads the result, derives what follows from it
//! (the PSRAM and flash sizes from the module, `runtime-psram`, the rules of
//! each of the chip's harts) and checks it. Nothing here touches hardware or the
//! file system: a [`Source`] reads the files.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};

use serde::Deserialize;
use toml::{Table, Value};

pub mod generate;
mod irq_stack;
mod layers;
mod model;
mod rom;
pub mod tracked;

pub use layers::{Layers, Source, chain, resolve};
pub use model::*;

/// Format of every layer file.
pub const SCHEMA: u32 = 2;

/// The keys of a chip layer that hold the image registry, not facts
/// (`oer-chip-profile` reads them): a caller takes them out of the resolved
/// table before [`Facts::from_table`].
pub const REGISTRY: &[&str] = &["hil", "packages", "probe"];

/// Why the facts are refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error(String);

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for Error {}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.to_string())
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// The platform data directory of chip `id`, relative to the root: its
/// chip layer, modules, boards, stack policy, partitions and ROM function
/// summaries.
pub fn platform_dir(id: &str) -> String {
    format!("chips/{id}/platform")
}

/// The chip layer of `id`, relative to the root.
pub fn chip_file(id: &str) -> String {
    format!("{}/chip.toml", platform_dir(id))
}

/// A chip or family identifier: a lowercase ASCII letter, then lowercase
/// letters, digits or hyphens.
pub fn valid_identifier(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// The top layer of chip `id`: its board, or its chip layer for a chip
/// without one. A chip has at most one board until its images name theirs.
pub fn top_layer(source: &mut dyn Source, id: &str) -> Result<String> {
    let directory = format!("{}/boards", platform_dir(id));
    let mut boards: Vec<String> = source
        .files(&directory)?
        .into_iter()
        .filter(|name| name.ends_with(".toml"))
        .collect();
    boards.sort();
    match boards.as_slice() {
        [] => Ok(chip_file(id)),
        [board] => Ok(format!("{directory}/{board}")),
        boards => Err(format!(
            "chip `{id}` has boards {}; an image names none yet, so a chip has one",
            boards.join(", ")
        )
        .into()),
    }
}

/// The resolved layers of chip `id`, from its top layer down; the chain
/// includes the chip's own layer.
pub fn load(source: &mut dyn Source, id: &str) -> Result<Layers> {
    let top = top_layer(source, id)?;
    let layers = resolve(source, &top)?;
    let chip = chip_file(id);
    if !layers.files.contains(&chip) {
        return Err(format!("{top} does not extend {chip}").into());
    }
    Ok(layers)
}

/// The resolved facts of one chip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Facts {
    /// The chip id: lowercase, as in paths, package names and evidence.
    pub id: String,
    /// The family whose shared code the chip uses: a `family` package
    /// declaring this id builds for, and may be used by, every chip of it.
    pub family: String,
    /// The ISA of the chip's harts, such as `rv32imafc`.
    pub isa: String,
    /// The Rust target triple of its firmware: the ISA's.
    pub rust_target: String,
    /// How the chip's images start: [`Boot::Staged`].
    pub boot: Boot,
    /// The `--chip` value of `espflash`.
    pub espflash_chip: String,
    /// Silicon revisions the repository's models and pins describe.
    pub revisions: Vec<String>,
    /// What the chip has.
    pub properties: Properties,
    /// Where the chip's images lie in flash: the image pipeline encodes
    /// for it, and every flash writes there. `None` for a chip that has no
    /// images yet.
    pub flash: Option<FlashMap>,
    /// The address map images are checked against; `None` for a chip whose
    /// images are not checked by address.
    pub memory: Option<Memory>,
    /// The interrupt contract the interrupt-stack check reads.
    pub interrupts: Option<InterruptContract>,
    /// The ROM the stack analyses read; `None` for a chip without a pinned
    /// ROM ELF.
    pub rom: Option<Rom>,
    /// The placement contract the placement check reads, with the rules of
    /// the chip's harts.
    pub placement: Option<Placement>,
    /// How code reaches a core other than core 0; a chip with more than one
    /// core names it.
    pub core_start: Option<CoreStart>,
    /// The staged boot's host contract; a chip with images names it.
    pub staged: Option<StagedBoot>,
    /// What the pinned second-stage bootloader keeps for itself.
    pub boot_chain: Option<BootChain>,
    /// Where the host reads why the chip last reset; `None` for a chip whose
    /// stand cannot prove a power loss.
    pub reset_cause: Option<ResetCause>,
    /// Demangled function-name patterns the reachability report of every
    /// image watches.
    pub observed_functions: Vec<String>,
    /// The module that carries the chip.
    pub module: Option<Module>,
    /// The board that carries the module.
    pub board: Option<Board>,
}

/// The facts as the layers state them, before what follows from them.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct Stated {
    id: String,
    family: String,
    isa: String,
    rust_target: String,
    boot: Boot,
    espflash_chip: String,
    revisions: Vec<String>,
    properties: Properties,
    #[serde(default)]
    flash: Option<StatedFlash>,
    #[serde(default)]
    memory: Option<StatedMemory>,
    #[serde(default)]
    interrupts: Option<InterruptContract>,
    #[serde(default)]
    rom: Option<Rom>,
    #[serde(default)]
    placement: Option<Placement>,
    #[serde(default)]
    core_start: Option<CoreStart>,
    #[serde(default)]
    staged: Option<StagedBoot>,
    #[serde(default)]
    boot_chain: Option<BootChain>,
    #[serde(default)]
    reset_cause: Option<ResetCause>,
    #[serde(default)]
    observed_functions: Vec<String>,
    #[serde(default)]
    module: Option<Module>,
    #[serde(default)]
    board: Option<Board>,
}

/// The chip's `[flash]`: the module states the flash's size.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct StatedFlash {
    bootloader: u32,
    partition_table: u32,
    application: u32,
    otadata: u32,
    partitions: String,
    application_encoding: StatedEncoding,
    bootloader_mode: FlashMode,
    start: Start,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct StatedEncoding {
    mode: FlashMode,
    frequency_mhz: u32,
}

/// The chip's `[memory]`: the module states the PSRAM's size.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct StatedMemory {
    internal_sram: Region,
    flash_xip: Region,
    #[serde(default)]
    psram_origin: Option<u32>,
    #[serde(default)]
    retained: Option<Region>,
    #[serde(default)]
    cache_window: Option<Region>,
    task_stacks: String,
    attributes: alloc::collections::BTreeMap<String, Attributes>,
}

impl Facts {
    /// The facts of chip `id` from its resolved `table` (whose top layer is
    /// `origin`, for messages): every fact key, and no other.
    pub fn from_table(id: &str, origin: &str, mut table: Table) -> Result<Self> {
        let at = |error: Error| Error::from(format!("{origin}: {error}"));
        expand_rules(&mut table).map_err(at)?;
        let stated: Stated = table
            .try_into()
            .map_err(|error| at(format!("{error}").into()))?;
        Self::derive(stated)
            .and_then(|facts| facts.check(id))
            .map_err(at)
    }

    fn derive(stated: Stated) -> Result<Self> {
        let module = stated.module;
        let flash = stated
            .flash
            .map(|flash| {
                let module = module.as_ref().ok_or(
                    "`[flash]` encodes the module's flash size, and no layer names a `[module]`",
                )?;
                Ok::<_, Error>(FlashMap {
                    bootloader: flash.bootloader,
                    partition_table: flash.partition_table,
                    application: flash.application,
                    otadata: flash.otadata,
                    partitions: flash.partitions,
                    application_encoding: FlashSettings {
                        mode: flash.application_encoding.mode,
                        frequency_mhz: flash.application_encoding.frequency_mhz,
                        size_mib: module.flash.size_mib,
                    },
                    bootloader_mode: flash.bootloader_mode,
                    start: flash.start,
                })
            })
            .transpose()?;
        let psram_bytes = module
            .as_ref()
            .and_then(|module| module.psram)
            .map(|psram| {
                psram
                    .size_mib
                    .checked_mul(1 << 20)
                    .ok_or_else(|| Error::from("the module's PSRAM size overflows u32"))
            })
            .transpose()?;
        let memory = stated
            .memory
            .map(|memory| {
                let psram = match (memory.psram_origin, psram_bytes) {
                    (Some(origin), Some(length)) => {
                        origin
                            .checked_add(length)
                            .ok_or("the module's PSRAM ends past the address space")?;
                        Some(Region { origin, length })
                    }
                    (None, None) => None,
                    (Some(_), None) => {
                        return Err(Error::from(
                            "`[memory] psram-origin` maps a PSRAM the module does not fit",
                        ));
                    }
                    (None, Some(_)) => {
                        return Err(Error::from(
                            "the module fits PSRAM, and `[memory]` names no `psram-origin`",
                        ));
                    }
                };
                let runtime_psram = match (psram, &stated.staged) {
                    (Some(psram), Some(staged)) => {
                        let probe = staged.bootstrap_psram_bytes;
                        if probe >= psram.length {
                            return Err(format!(
                                "the bootstrap's {probe:#x}-byte PSRAM probe page leaves no \
                                 room in {:#x} bytes of PSRAM",
                                psram.length
                            )
                            .into());
                        }
                        Some(Region {
                            origin: psram.origin + probe,
                            length: psram.length - probe,
                        })
                    }
                    _ => None,
                };
                let internal = memory.internal_sram;
                let sram = match &stated.boot_chain {
                    Some(chain) => {
                        let end = chain.application_sram_end;
                        if end <= internal.origin || end > internal.end() {
                            return Err(format!(
                                "`[boot-chain] application-sram-end` {end:#010x} must lie in the \
                                 internal SRAM {:#010x}..{:#010x}",
                                internal.origin,
                                internal.end()
                            )
                            .into());
                        }
                        Region {
                            origin: internal.origin,
                            length: end - internal.origin,
                        }
                    }
                    None => internal,
                };
                Ok(Memory {
                    internal_sram: internal,
                    sram,
                    flash_xip: memory.flash_xip,
                    psram,
                    runtime_psram,
                    retained: memory.retained,
                    cache_window: memory.cache_window,
                    task_stacks: memory.task_stacks,
                    attributes: memory.attributes,
                })
            })
            .transpose()?;
        Ok(Self {
            id: stated.id,
            family: stated.family,
            isa: stated.isa,
            rust_target: stated.rust_target,
            boot: stated.boot,
            espflash_chip: stated.espflash_chip,
            revisions: stated.revisions,
            properties: stated.properties,
            flash,
            memory,
            interrupts: stated.interrupts,
            rom: stated.rom,
            placement: stated.placement,
            core_start: stated.core_start,
            staged: stated.staged,
            boot_chain: stated.boot_chain,
            reset_cause: stated.reset_cause,
            observed_functions: stated.observed_functions,
            module,
            board: stated.board,
        })
    }

    fn check(self, id: &str) -> Result<Self> {
        if self.id != id {
            return Err(format!("the layers name chip `{}`", self.id).into());
        }
        if self.boot != Boot::Staged {
            return Err(format!(
                "names boot `{:?}`; a chip's images boot staged, the ESP-IDF bootloader \
                 boots only the firmware catalog's images",
                self.boot
            )
            .into());
        }
        if !valid_identifier(&self.family) {
            return Err(format!("names invalid family `{}`", self.family).into());
        }
        let target = self
            .isa
            .strip_prefix("rv32i")
            .map(|extensions| format!("riscv32i{extensions}-unknown-none-elf"));
        if target.as_deref() != Some(self.rust_target.as_str()) {
            return Err(format!(
                "Rust target `{}` is not the bare-metal target of ISA `{}`",
                self.rust_target, self.isa
            )
            .into());
        }
        if let Some(interrupts) = &self.interrupts {
            interrupts.validate()?;
        }
        if let Some(memory) = &self.memory {
            memory.validate(self.boot_chain.as_ref(), self.rom.as_ref())?;
        }
        if let Some(rom) = &self.rom {
            rom.validate_functions()?;
        }
        if let Some(rom) = &self.rom
            && !self.revisions.contains(&rom.revision)
        {
            return Err(format!(
                "the ROM of revision `{}` is not of a revision the chip names",
                rom.revision
            )
            .into());
        }
        if (self.properties.cores > 1) != self.core_start.is_some() {
            return Err(
                "`[core-start]` is named by exactly the chips with more than one core".into(),
            );
        }
        let mut observed = alloc::collections::BTreeSet::new();
        for pattern in &self.observed_functions {
            if pattern.trim().is_empty() || !observed.insert(pattern) {
                return Err(
                    format!("empty or duplicate observed function pattern: {pattern:?}").into(),
                );
            }
        }
        Ok(self)
    }
}

/// Expand the placement rules of the chip's harts and lines: a rule with
/// `per-hart = true` stands once for each hart, `{hart}` replaced by its
/// number, a rule with `bytes-per-hart` has `bytes` of that many for each
/// hart, and a rule with `per-line = true` numbers `{n}` by every vector-entry
/// line, 1 to `[interrupts] lines` less one.
fn expand_rules(table: &mut Table) -> Result<()> {
    let interrupts = table.get("interrupts");
    let harts: Option<Vec<i64>> = interrupts
        .and_then(|interrupts| interrupts.get("harts"))
        .and_then(Value::as_array)
        .map(|harts| harts.iter().filter_map(Value::as_integer).collect());
    let lines = interrupts
        .and_then(|interrupts| interrupts.get("lines"))
        .and_then(Value::as_integer);
    let Some(Value::Table(placement)) = table.get_mut("placement") else {
        return Ok(());
    };
    for kind in ["range", "symbol", "stack-swap"] {
        let Some(Value::Array(rules)) = placement.get_mut(kind) else {
            continue;
        };
        let mut expanded = Vec::with_capacity(rules.len());
        for mut rule in core::mem::take(rules) {
            let Some(fields) = rule.as_table_mut() else {
                expanded.push(rule);
                continue;
            };
            match fields.remove("per-line") {
                None => {}
                Some(Value::Boolean(true)) => {
                    if fields.contains_key("numbers") {
                        return Err(format!(
                            "a per-line `[placement] {kind}` rule takes its numbers from the lines"
                        )
                        .into());
                    }
                    let last = lines.filter(|&lines| lines > 1).ok_or_else(|| {
                        Error::from(format!(
                            "a per-line `[placement] {kind}` rule needs `[interrupts] lines`"
                        ))
                    })? - 1;
                    fields.insert(
                        "numbers".to_string(),
                        Value::Array(alloc::vec![Value::Integer(1), Value::Integer(last)]),
                    );
                }
                Some(_) => {
                    return Err(
                        format!("a `[placement] {kind}` rule's `per-line` is not `true`").into(),
                    );
                }
            }
            let per_hart = fields.remove("per-hart");
            let bytes_per_hart = fields.remove("bytes-per-hart");
            if per_hart.is_none() && bytes_per_hart.is_none() {
                expanded.push(rule);
                continue;
            }
            let harts = harts.as_ref().ok_or_else(|| {
                Error::from(format!(
                    "a per-hart `[placement] {kind}` rule needs `[interrupts] harts`"
                ))
            })?;
            if let Some(bytes) = bytes_per_hart {
                let bytes = bytes
                    .as_integer()
                    .and_then(|bytes| bytes.checked_mul(i64::try_from(harts.len()).ok()?))
                    .ok_or_else(|| {
                        Error::from(format!(
                            "a `[placement] {kind}` rule's `bytes-per-hart` is not a size"
                        ))
                    })?;
                fields.insert("bytes".to_string(), Value::Integer(bytes));
            }
            match per_hart {
                None => expanded.push(rule),
                Some(Value::Boolean(true)) => {
                    for hart in harts {
                        expanded.push(with_hart(&rule, &format!("{hart}")));
                    }
                }
                Some(_) => {
                    return Err(
                        format!("a `[placement] {kind}` rule's `per-hart` is not `true`").into(),
                    );
                }
            }
        }
        *rules = expanded;
    }
    Ok(())
}

/// `value` with `{hart}` replaced by `hart` in every string.
fn with_hart(value: &Value, hart: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("{hart}", hart)),
        Value::Array(values) => {
            Value::Array(values.iter().map(|value| with_hart(value, hart)).collect())
        }
        Value::Table(table) => Value::Table(
            table
                .iter()
                .map(|(key, value)| (key.clone(), with_hart(value, hart)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The tracked facts of `id`.
    pub(crate) fn tracked(id: &str) -> Facts {
        let layers = load(&mut tracked::Tracked, id).expect("the tracked layers resolve");
        let mut table = layers.table.clone();
        for registry in REGISTRY {
            table.remove(*registry);
        }
        Facts::from_table(id, layers.top(), table).expect("the tracked facts load")
    }

    fn facts(text: &str) -> Result<Facts> {
        let table: Table = toml::from_str(text).expect("valid TOML");
        Facts::from_table("chip-x", "chip.toml", table)
    }

    const CHIP: &str = "id = 'chip-x'\nfamily = 'f'\nisa = 'rv32imac'\n\
        rust-target = 'riscv32imac-unknown-none-elf'\nboot = 'staged'\nespflash-chip = 'x'\n\
        revisions = []\n[properties]\nwifi-bands = []\nbluetooth = []\nieee802154 = false\n\
        cores = 1\n";

    #[test]
    fn the_tracked_chips_resolve_from_their_boards() {
        for (id, board, module, revision) in [(
            "esp32s31",
            "ESP32-S31-Function-CoreBoard-1",
            "ESP32-S31-WROOM-3-N16R16",
            "rev0",
        )] {
            let layers = load(&mut tracked::Tracked, id).expect("the layers resolve");
            assert_eq!(
                layers.files.len(),
                6,
                "{id}: ISA, family, chip, ROM, module, board"
            );
            assert_eq!(
                layers.files[3],
                format!("chips/{id}/platform/rom-{revision}.toml")
            );
            assert!(layers.top().contains("/boards/"), "{id}");
            let facts = tracked(id);
            assert_eq!(
                facts.board.as_ref().map(|board| board.name.as_str()),
                Some(board)
            );
            assert_eq!(
                facts.module.as_ref().map(|module| module.name.as_str()),
                Some(module)
            );
            assert_eq!(facts.family, "espressif");
            assert!(!facts.observed_functions.is_empty());
            assert_eq!(
                facts.rom.as_ref().map(|rom| rom.revision.as_str()),
                Some(revision)
            );
        }
    }

    /// The application's SRAM as the layouts have linked it, from the
    /// internal SRAM and the boot chain's reserve, clear of the ROM's data.
    #[test]
    fn the_application_sram_is_the_internal_sram_below_the_reserves() {
        for (id, sram, data) in [(
            "esp32s31",
            Region {
                origin: 0x2f00_0000,
                length: 0x0007_afc0,
            },
            0x2f07_e600,
        )] {
            let facts = tracked(id);
            let memory = facts.memory.as_ref().expect("memory");
            assert_eq!(memory.sram, sram, "{id}");
            assert_eq!(memory.internal_sram.origin, sram.origin, "{id}");
            let rom = facts.rom.as_ref().expect("rom");
            let running: Vec<_> = rom
                .sram
                .iter()
                .filter(|range| range.kept == Kept::Running)
                .collect();
            assert_eq!(running.len(), 1, "{id}");
            assert_eq!(
                (running[0].start, running[0].end),
                (data, memory.internal_sram.end()),
                "{id}"
            );
        }
    }

    #[test]
    fn a_rom_range_kept_while_an_image_runs_is_refused_in_the_application_sram() {
        let s31 = tracked("esp32s31");
        let memory = s31.memory.clone().expect("memory");
        let chain = s31.boot_chain.clone().expect("boot chain");
        let rom = s31.rom.clone().expect("rom");
        assert!(memory.validate(Some(&chain), Some(&rom)).is_ok());

        let mut overlapping = rom.clone();
        let data = overlapping
            .sram
            .iter_mut()
            .find(|range| range.kept == Kept::Running)
            .expect("the ROM's data");
        data.start = memory.sram.end() - 4;
        // Ranges stay in address order: the boot stack ends where the data
        // now starts.
        overlapping.sram.retain(|range| range.kept != Kept::Boot);
        overlapping
            .sram
            .retain(|range| range.kept != Kept::Download);
        let error = memory
            .validate(Some(&chain), Some(&overlapping))
            .expect_err("ROM data in the application's SRAM");
        assert!(error.to_string().contains("while an image runs"), "{error}");

        // A boot stack or a download buffer the image reuses is no conflict.
        let mut reused = rom.clone();
        for range in &mut reused.sram {
            if range.kept != Kept::Running {
                range.start = range.start.min(memory.sram.end() - 0x100);
            }
        }
        reused.sram.retain(|range| range.kept != Kept::Download);
        assert!(memory.validate(Some(&chain), Some(&reused)).is_ok());

        let mut outside = rom.clone();
        outside.sram[2].end = memory.internal_sram.end() + 4;
        assert!(memory.validate(Some(&chain), Some(&outside)).is_err());
        let mut unordered = rom;
        unordered.sram.swap(0, 2);
        assert!(memory.validate(Some(&chain), Some(&unordered)).is_err());
    }

    #[test]
    fn the_application_sram_ends_at_the_boot_chain_s_reserve() {
        let base = format!(
            "{CHIP}[memory]\ninternal-sram = {{ origin = 0x1000, length = 0x1000 }}\n\
             flash-xip = {{ origin = 0x10000, length = 0x1000 }}\n\
             retained = {{ origin = 0x20000, length = 0x100 }}\ntask-stacks = 'sram'\n\
             [memory.attributes]\n\
             sram = {{ executable = true, cached = false, dma = true, retained = false, writable = true }}\n\
             retained = {{ executable = true, cached = false, dma = false, retained = true, writable = true }}\n\
             flash-xip = {{ executable = true, cached = true, dma = false, retained = false, writable = false }}\n"
        );
        let chain = |end: u32| {
            format!(
                "{base}[boot-chain]\napplication-sram-end = {end:#x}\n\
                 second-stage-loader-start = 0x1800\nretained-reserved-tail = 24\n"
            )
        };
        let reserved = facts(&chain(0x1c00)).expect("a reserve in the internal SRAM");
        assert_eq!(
            reserved.memory.expect("memory").sram,
            Region {
                origin: 0x1000,
                length: 0xc00
            }
        );
        for end in [0x1000, 0x2004] {
            let error = facts(&chain(end)).expect_err("a reserve outside the internal SRAM");
            assert!(
                error.to_string().contains("application-sram-end"),
                "{error}"
            );
        }
        let whole = facts(&base).expect("no boot chain");
        assert_eq!(
            whole.memory.expect("memory").sram,
            Region {
                origin: 0x1000,
                length: 0x1000
            }
        );
        let unattributed = base.replace("flash-xip = { executable", "flash = { executable");
        let error = facts(&unattributed).expect_err("an unmapped region's attributes");
        assert!(error.to_string().contains("memory.attributes"), "{error}");
    }

    #[test]
    fn the_rom_is_of_a_revision_the_chip_names() {
        let rom = "[rom]\nrevision = 'rev1'\nelf = 'rom'\nlinker-script = 'rom.x'\n";
        let error = facts(&format!("{CHIP}{rom}")).expect_err("no such revision");
        assert!(error.to_string().contains("`rev1`"), "{error}");
        let named = CHIP.replace("revisions = []", "revisions = ['rev1']");
        assert!(facts(&format!("{named}{rom}")).is_ok());
    }

    #[test]
    fn the_checks_read_a_region_s_attributes() {
        let s31 = tracked("esp32s31");
        let memory = s31.memory.as_ref().expect("memory");
        let cached: Vec<_> = memory
            .regions()
            .into_iter()
            .filter(|mapped| mapped.attributes.cached)
            .map(|mapped| mapped.name)
            .collect();
        assert_eq!(cached, ["flash-xip", "runtime-psram", "psram"]);
        let sram = memory.sram;
        let at = memory
            .region_of(u64::from(sram.origin), u64::from(sram.origin) + 4)
            .expect("in SRAM");
        assert_eq!(at.name, "sram");
        assert!(at.attributes.dma && !at.attributes.cached);
        let runtime = memory.runtime_psram.expect("runtime PSRAM");
        let at = memory
            .region_of(u64::from(runtime.origin), u64::from(runtime.origin) + 4)
            .expect("in PSRAM");
        assert_eq!(at.name, "runtime-psram");
        assert!(at.attributes.cached && at.attributes.writable);
        assert_eq!(memory.region("retained"), memory.retained);
        assert!(
            memory
                .region_of(u64::from(sram.end()), u64::from(sram.end()) + 4)
                .is_none(),
            "the ROM's reserve is no region of the map"
        );
    }

    #[test]
    fn the_module_sizes_the_flash_and_the_psram_and_the_probe_page_starts_the_runtime() {
        let s31 = tracked("esp32s31");
        let memory = s31.memory.as_ref().expect("memory");
        assert_eq!(
            memory.psram,
            Some(Region {
                origin: 0x5000_0000,
                length: 16 << 20
            })
        );
        assert_eq!(
            memory.runtime_psram,
            Some(Region {
                origin: 0x5001_0000,
                length: (16 << 20) - 0x1_0000
            })
        );
        assert_eq!(
            s31.flash
                .as_ref()
                .map(|flash| flash.application_encoding.size_mib),
            Some(16)
        );
    }

    #[test]
    fn a_per_hart_rule_stands_for_each_hart_of_the_chip() {
        let placement = |id: &str| tracked(id).placement.expect("placement");
        let s31 = placement("esp32s31");
        let names = |placement: &Placement| -> Vec<String> {
            placement
                .symbol
                .iter()
                .map(|rule| rule.name.clone())
                .collect()
        };
        assert_eq!(
            names(&s31),
            [
                "_start_trap",
                "_runtime_irq_h0_l{n}",
                "_runtime_irq_h1_l{n}",
                "_runtime_irq_exit",
                "_runtime_mtvt",
                "__rustc::rust_begin_unwind",
                "oer_platform_panic_hook",
            ]
        );
        let mtvt = s31
            .symbol
            .iter()
            .find(|rule| rule.name == "_runtime_mtvt")
            .map(|rule| rule.bytes);
        assert_eq!(mtvt, Some(512));
        let stacks = |placement: &Placement| -> Vec<String> {
            placement
                .range
                .iter()
                .filter(|rule| rule.maximum_bytes.is_some())
                .map(|rule| rule.range[1].clone())
                .collect()
        };
        assert_eq!(
            stacks(&s31),
            [
                "__runtime_cpu0_irq_stack_top",
                "__runtime_cpu1_irq_stack_top"
            ]
        );
        assert_eq!(s31.stack_swap.len(), 3);

        let error = facts(&format!(
            "{CHIP}[placement]\nname = 'p'\n\
             symbol = [{{ name = 's{{hart}}', bytes = 4, region = 'sram', per-hart = true }}]\n"
        ))
        .expect_err("a per-hart rule needs harts");
        assert!(
            error.to_string().contains("needs `[interrupts] harts`"),
            "{error}"
        );
    }

    #[test]
    fn a_per_line_rule_numbers_every_vector_entry_line() {
        for id in ["esp32s31"] {
            let placement = tracked(id).placement.expect("placement");
            let entries = placement
                .symbol
                .iter()
                .find(|rule| rule.name == "_runtime_irq_h0_l{n}")
                .expect("hart 0's entries");
            assert_eq!(entries.numbers, Some([1, 47]), "{id}");
            assert!(
                placement
                    .stack_swap
                    .iter()
                    .any(|rule| rule.numbers == Some([1, 47])),
                "{id}"
            );
        }
        let rule = |extra: &str| {
            format!(
                "{CHIP}[interrupts]\nlines = 4\n[placement]\nname = 'p'\n\
                 symbol = [{{ name = 'e{{n}}', bytes = 4, region = 'sram', per-line = true{extra} }}]\n"
            )
        };
        let table: Table = toml::from_str(&rule("")).expect("valid TOML");
        let mut table = table;
        expand_rules(&mut table).expect("expands");
        assert_eq!(
            table["placement"]["symbol"][0]["numbers"],
            Value::Array(alloc::vec![Value::Integer(1), Value::Integer(3)])
        );
        let mut doubled: Table = toml::from_str(&rule(", numbers = [1, 2]")).expect("valid");
        assert!(expand_rules(&mut doubled).is_err());
        let mut no_lines: Table =
            toml::from_str(&rule("").replace("lines = 4\n", "")).expect("valid");
        assert!(expand_rules(&mut no_lines).is_err());
    }

    #[test]
    fn flash_and_psram_lie_in_the_shared_cache_window_and_the_loader_in_sram() {
        let s31 = tracked("esp32s31");
        let memory = s31.memory.clone().expect("memory");
        assert!(memory.validate(s31.boot_chain.as_ref(), None).is_ok());
        let mut narrow = memory.clone();
        narrow.cache_window = Some(Region {
            origin: 0x4200_0000,
            length: 0x0100_0000,
        });
        assert!(
            narrow.validate(None, None).is_err(),
            "PSRAM outside the window"
        );
        let mut chain = s31.boot_chain.clone().expect("boot chain");
        chain.second_stage_loader_start = memory.sram.end() + 4;
        assert!(memory.validate(Some(&chain), None).is_err());
        let mut chain = s31.boot_chain.clone().expect("boot chain");
        chain.retained_reserved_tail = 0x1_0000;
        assert!(memory.validate(Some(&chain), None).is_err());
    }

    #[test]
    fn a_missing_key_is_refused_by_name() {
        let without_target = CHIP.replace("rust-target = 'riscv32imac-unknown-none-elf'\n", "");
        let error = facts(&without_target).expect_err("a required key is missing");
        assert!(error.to_string().contains("rust-target"), "{error}");
        assert!(error.to_string().starts_with("chip.toml: "), "{error}");
        let unknown =
            facts(&format!("{CHIP}colour = 'blue'\n")).expect_err("an unknown key is refused");
        assert!(unknown.to_string().contains("colour"), "{unknown}");
    }

    #[test]
    fn the_flash_and_the_psram_need_the_module() {
        let flash = format!(
            "{CHIP}[flash]\nbootloader = 0\npartition-table = 1\napplication = 2\notadata = 3\n\
             partitions = 'p.csv'\napplication-encoding = {{ mode = 'qio', frequency-mhz = 80 }}\n\
             bootloader-mode = 'dio'\nstart = 'reset'\n"
        );
        let error = facts(&flash.clone()).expect_err("no module");
        assert!(error.to_string().contains("`[module]`"), "{error}");
        let with_module = format!("{flash}[module]\nname = 'm'\nflash = {{ size-mib = 4 }}\n");
        let facts_with = facts(&with_module).expect("a module");
        assert_eq!(
            facts_with
                .flash
                .map(|flash| flash.application_encoding.size_mib),
            Some(4)
        );

        let memory = format!(
            "{CHIP}[memory]\ninternal-sram = {{ origin = 0, length = 16 }}\n\
             flash-xip = {{ origin = 16, length = 16 }}\npsram-origin = 64\ntask-stacks = 'sram'\n\
             attributes = {{}}\n"
        );
        let error = facts(&memory).expect_err("no PSRAM fitted");
        assert!(error.to_string().contains("psram-origin"), "{error}");
    }

    #[test]
    fn the_rust_target_is_the_isa_s() {
        let wrong = CHIP.replace("riscv32imac-unknown", "riscv32imafc-unknown");
        let error = facts(&wrong).expect_err("another ISA's target");
        assert!(error.to_string().contains("ISA `rv32imac`"), "{error}");
        assert!(facts(CHIP).is_ok());
    }

    #[test]
    fn a_chip_has_at_most_one_board() {
        struct Boards(Vec<String>);
        impl Source for Boards {
            fn read(&mut self, path: &str) -> Result<String> {
                Err(format!("unread {path}").into())
            }
            fn files(&mut self, _: &str) -> Result<Vec<String>> {
                Ok(self.0.clone())
            }
        }
        let mut none = Boards(Vec::new());
        assert_eq!(
            top_layer(&mut none, "x").as_deref(),
            Ok("chips/x/platform/chip.toml")
        );
        let mut one = Boards(alloc::vec!["b.toml".into(), "README.md".into()]);
        assert_eq!(
            top_layer(&mut one, "x").as_deref(),
            Ok("chips/x/platform/boards/b.toml")
        );
        let mut two = Boards(alloc::vec!["b.toml".into(), "a.toml".into()]);
        let error = top_layer(&mut two, "x").expect_err("two boards");
        assert!(error.to_string().contains("a.toml, b.toml"), "{error}");
    }

    #[test]
    fn a_board_that_does_not_reach_the_chip_is_refused() {
        struct Stray;
        impl Source for Stray {
            fn read(&mut self, path: &str) -> Result<String> {
                match path {
                    "chips/x/platform/boards/b.toml" => Ok("schema = 2\n".into()),
                    _ => Err(format!("no {path}").into()),
                }
            }
            fn files(&mut self, _: &str) -> Result<Vec<String>> {
                Ok(alloc::vec!["b.toml".into()])
            }
        }
        let error = load(&mut Stray, "x").expect_err("the board extends nothing");
        assert!(
            error
                .to_string()
                .contains("does not extend chips/x/platform/chip.toml"),
            "{error}"
        );
    }
}
