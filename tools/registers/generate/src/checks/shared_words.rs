//! No MMIO word has two uncoordinated writers.
//!
//! The radio PAC and esp-hal each own registers of one chip. A word that
//! both can store to is a race: a read-modify-write on one side can undo the
//! other's store. This check lists every word a write transaction of the
//! closed radio PAC stores to (`registers/<chip>/policy/api.toml`, located
//! through `registers/<chip>/published/radio.svd`) that pinned esp-hal also
//! writes (a `PERIPHERAL::regs().register().write/modify` in its sources,
//! resolved to an address through the pinned platform PAC), and requires each
//! to have a reviewed entry in `registers/<chip>/shared-words.toml` saying how
//! the two writers are coordinated. A reviewed word that is no longer shared
//! is rejected too, so the list stays exact. There is no pending list: a new
//! shared word is fixed or reviewed in the change that introduces it.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::Result;

/// How a word's two writers are kept from racing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Coordination {
    /// Both writers store only while holding the named lock.
    SharedLock,
    /// esp-hal stores only at points where no radio owner of the word
    /// exists (initialization, handoff, sleep entry with the radio stopped).
    DisjointLifecycle,
    /// esp-hal's storing function exists but no image reaches it (no caller,
    /// or its clock-tree node is not configured); the reason names it, and a
    /// merge that starts calling it must revisit the entry.
    UnreachedEspHalPath,
    /// Both sides store whole words (trigger strobes); neither reads,
    /// modifies and writes back, so neither undoes the other.
    WholeWordStores,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    schema: u32,
    #[serde(default)]
    word: Vec<ReviewedWord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedWord {
    address: u32,
    radio: String,
    esp_hal: String,
    coordination: Coordination,
    reason: String,
}

/// The trimmed text of `node`'s child element `name`.
fn text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .map(str::trim)
}

/// Absolute word addresses of every register of the radio SVD, by
/// `(peripheral, register)`; an array register lists all its elements.
pub fn radio_registers(svd: &str) -> Result<BTreeMap<(String, String), Vec<u32>>> {
    let document = roxmltree::Document::parse(svd)?;
    let number = |value: &str| -> Result<u32> {
        let value = value.trim();
        Ok(
            match value.strip_prefix("0x").or(value.strip_prefix("0X")) {
                Some(hex) => u32::from_str_radix(hex, 16)?,
                None => value.parse()?,
            },
        )
    };
    let mut registers = BTreeMap::new();
    for peripheral in document
        .descendants()
        .filter(|node| node.has_tag_name("peripheral"))
    {
        let base =
            number(text(peripheral, "baseAddress").ok_or("peripheral without baseAddress")?)?;
        let peripheral_name = text(peripheral, "name").unwrap_or_default().to_owned();
        for register in peripheral
            .descendants()
            .filter(|node| node.has_tag_name("register"))
        {
            let offset =
                number(text(register, "addressOffset").ok_or("register without addressOffset")?)?;
            let count = text(register, "dim").map(number).transpose()?.unwrap_or(1);
            let increment = text(register, "dimIncrement")
                .map(number)
                .transpose()?
                .unwrap_or(4);
            registers.insert(
                (
                    peripheral_name.clone(),
                    text(register, "name").unwrap_or_default().to_owned(),
                ),
                (0..count)
                    .map(|index| base + offset + index * increment)
                    .collect(),
            );
        }
    }
    Ok(registers)
}

/// API policy tables whose transactions store to their registers.
const WRITE_TRANSACTIONS: &[&str] = &[
    "field-argument-modifies",
    "field-or-modifies",
    "field-replace-modifies",
    "fixed-register-images",
    "fixed-register-sequences",
    "fixed-register-writes",
    "full-register-writes",
    "indexed-bit-set-modifies",
    "interrupt-snapshots",
    "masked-register-modifies",
    "register-image-writes",
    "sampled-bit-zero-writes",
    "w1c-register-snapshots",
    "zero-based-field-writes",
    "zero-register-writes",
];

/// `(peripheral, register)` pairs a write transaction of the API policy
/// stores to, including sequence steps and interrupt clear registers.
pub fn api_write_registers(api: &str) -> Result<BTreeSet<(String, String)>> {
    let policy: toml::Table = toml::from_str(api)?;
    let mut registers = BTreeSet::new();
    for kind in WRITE_TRANSACTIONS {
        let Some(transactions) = policy.get(*kind).and_then(toml::Value::as_array) else {
            continue;
        };
        for transaction in transactions {
            let Some(peripheral) = transaction.get("peripheral").and_then(toml::Value::as_str)
            else {
                continue;
            };
            let steps = transaction.get("steps").and_then(toml::Value::as_array);
            let named = ["register", "clear-register"]
                .iter()
                .filter_map(|key| transaction.get(*key))
                .chain(
                    steps
                        .into_iter()
                        .flatten()
                        .filter_map(|step| step.get("register")),
                );
            for register in named.filter_map(toml::Value::as_str) {
                registers.insert((peripheral.to_owned(), register.to_owned()));
            }
        }
    }
    Ok(registers)
}

/// Words the radio PAC's write transactions store to, with register names.
pub fn radio_written(svd: &str, api: &str) -> Result<BTreeMap<u32, BTreeSet<String>>> {
    let registers = radio_registers(svd)?;
    let mut words: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (peripheral, register) in api_write_registers(api)? {
        let addresses = registers
            .get(&(peripheral.clone(), register.clone()))
            .ok_or_else(|| {
                format!("api policy writes {peripheral}.{register}, which the radio SVD lacks")
            })?;
        for address in addresses {
            words
                .entry(*address)
                .or_default()
                .insert(format!("{peripheral}.{register}"));
        }
    }
    Ok(words)
}

/// Peripheral base addresses and modules of the platform PAC's `lib.rs`
/// (`pub type NAME = crate::Periph<module::RegisterBlock, 0xADDR>;`).
pub fn pac_peripherals(lib: &str) -> BTreeMap<String, (String, u32)> {
    let mut peripherals = BTreeMap::new();
    for line in lib.lines() {
        let Some(rest) = line.trim().strip_prefix("pub type ") else {
            continue;
        };
        let Some((name, rest)) = rest.split_once(" = crate::Periph<") else {
            continue;
        };
        let Some((module, rest)) = rest.split_once("::RegisterBlock, ") else {
            continue;
        };
        let address = rest.trim_end_matches(">;").replace('_', "");
        if let Some(hex) = address.strip_prefix("0x")
            && let Ok(address) = u32::from_str_radix(hex, 16)
        {
            peripherals.insert(name.to_owned(), (module.to_owned(), address));
        }
    }
    peripherals
}

/// Register accessors of one generated register block with their byte
/// ranges: `#[doc = "0x04 - NAME"]` or `#[doc = "0x10..0x20 - NAME"]`
/// followed by `pub const fn accessor(`.
pub fn pac_registers(module: &str) -> BTreeMap<String, (u32, u32)> {
    let mut registers = BTreeMap::new();
    let mut range = None;
    for line in module.lines() {
        let line = line.trim();
        if let Some(doc) = line.strip_prefix("#[doc = \"0x") {
            let offsets = doc.split(" - ").next().unwrap_or_default();
            let parse = |hex: &str| u32::from_str_radix(hex.trim_start_matches("0x"), 16).ok();
            range = match offsets.split_once("..") {
                Some((start, end)) => parse(start).zip(parse(end)),
                None => parse(offsets).map(|start| (start, start + 4)),
            };
        } else if let Some(rest) = line.strip_prefix("pub const fn ") {
            if let (Some(range), Some(name)) = (range.take(), rest.split('(').next()) {
                registers.insert(name.to_owned(), range);
            }
        } else if !line.starts_with("#[") {
            range = None;
        }
    }
    registers
}

/// `(peripheral, register accessor)` pairs that esp-hal source stores to:
/// `PERIPHERAL::regs().register(..).write/modify/reset`, also through a
/// `let name = PERIPHERAL::regs();` binding, across line breaks.
pub fn esp_hal_stores(source: &str) -> BTreeSet<(String, String)> {
    let compact: String = source.split_whitespace().collect();
    let mut bindings = BTreeMap::new();
    for (index, _) in compact.match_indices("::regs();") {
        let before = &compact[..index];
        let peripheral = trailing_constant(before);
        if let Some(assignment) = before[..before.len() - peripheral.len()]
            .rsplit_once("let")
            .and_then(|(_, binding)| binding.split('=').next())
            .map(|binding| {
                binding
                    .trim_start_matches("mut")
                    .split(':')
                    .next()
                    .unwrap_or_default()
            })
            && !peripheral.is_empty()
            && assignment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            bindings.insert(assignment.to_owned(), peripheral.to_owned());
        }
    }
    let mut stores = BTreeSet::new();
    let mut rest = compact.as_str();
    while let Some(dot) = rest.find('.') {
        let receiver_end = dot;
        let head = &rest[..receiver_end];
        rest = &rest[dot + 1..];
        let peripheral = if let Some(stripped) = head.strip_suffix("::regs()") {
            trailing_constant(stripped).to_owned()
        } else {
            let name: String = head
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            match bindings.get(&name) {
                Some(peripheral) => peripheral.clone(),
                None => continue,
            }
        };
        if peripheral.is_empty() {
            continue;
        }
        let Some((register, after)) = call(rest) else {
            continue;
        };
        if ["write(", "modify(", "write_with_zero(", "reset()"]
            .iter()
            .any(|store| {
                after
                    .strip_prefix('.')
                    .is_some_and(|next| next.starts_with(store))
            })
        {
            stores.insert((peripheral, register.to_owned()));
        }
    }
    stores
}

/// The upper-case path segment ending `text`, such as `MODEM_LPCON`.
fn trailing_constant(text: &str) -> &str {
    let start = text
        .rfind(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .map_or(0, |index| index + 1);
    &text[start..]
}

/// A method call `name(args)` at the start of `text`: the name and the text
/// after its closing parenthesis.
fn call(text: &str) -> Option<(&str, &str)> {
    let open = text.find('(')?;
    let name = &text[..open];
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let mut depth = 0usize;
    for (index, character) in text[open..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((name, &text[open + index + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether an esp-hal source file can be compiled for `chip`: files and
/// directories named for another chip are skipped.
fn applies_to(relative: &Path, chip: &str) -> bool {
    relative.components().all(|component| {
        let name = component.as_os_str().to_string_lossy();
        let stem = name.split('.').next().unwrap_or_default();
        !(stem.starts_with("esp32") && stem != chip)
    })
}

fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, found)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
    Ok(())
}

/// Every esp-hal store resolved to absolute word addresses.
fn esp_hal_words(
    esp_hal_src: &Path,
    pac_src: &Path,
    chip: &str,
) -> Result<BTreeMap<u32, BTreeSet<String>>> {
    let peripherals = pac_peripherals(&fs::read_to_string(pac_src.join("lib.rs"))?);
    let mut modules = BTreeMap::new();
    let mut files = Vec::new();
    rust_files(esp_hal_src, &mut files)?;
    let mut words: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for file in files {
        let relative = file.strip_prefix(esp_hal_src)?;
        if !applies_to(relative, chip) {
            continue;
        }
        for (peripheral, register) in esp_hal_stores(&fs::read_to_string(&file)?) {
            let Some((module, base)) = peripherals.get(&peripheral) else {
                continue;
            };
            if !modules.contains_key(module) {
                let path = pac_src.join(format!("{module}.rs"));
                let registers = fs::read_to_string(path)
                    .map(|text| pac_registers(&text))
                    .unwrap_or_default();
                modules.insert(module.clone(), registers);
            }
            let Some(&(start, end)) = modules[module].get(&register) else {
                continue;
            };
            for address in (base + start..base + end).step_by(4) {
                words
                    .entry(address)
                    .or_default()
                    .insert(format!("{peripheral}.{register} ({})", relative.display()));
            }
        }
    }
    Ok(words)
}

/// Checks `chip`'s shared words against its review file.
pub fn check(root: &Path, chip: &str, esp_hal_src: &Path, pac_src: &Path) -> Result<usize> {
    let publication = crate::ChipSources::load(
        &root.join(format!("registers/{chip}/publication/registers.toml")),
    )?;
    let radio = radio_written(
        &fs::read_to_string(&publication.svd)?,
        &fs::read_to_string(&publication.api)?,
    )?;
    let esp_hal = esp_hal_words(esp_hal_src, pac_src, chip)?;
    let shared: BTreeSet<u32> = radio
        .keys()
        .filter(|address| esp_hal.contains_key(address))
        .copied()
        .collect();
    let review_path = root.join(format!("registers/{chip}/shared-words.toml"));
    let review: Review = toml_edit::de::from_str(
        &fs::read_to_string(&review_path).unwrap_or_else(|_| "schema = 1".into()),
    )?;
    if review.schema != 1 {
        return Err(format!(
            "{}: unsupported schema {}",
            review_path.display(),
            review.schema
        )
        .into());
    }
    let mut problems = Vec::new();
    let reviewed: BTreeMap<u32, &ReviewedWord> = review
        .word
        .iter()
        .map(|word| (word.address, word))
        .collect();
    for address in &shared {
        match reviewed.get(address) {
            None => problems.push(format!(
                "{address:#010x}: radio PAC {} and esp-hal {} both write it; make one side the only writer or review its coordination in {}",
                radio[address].iter().cloned().collect::<Vec<_>>().join(", "),
                esp_hal[address].iter().cloned().collect::<Vec<_>>().join(", "),
                review_path.display()
            )),
            Some(word) if word.reason.trim().is_empty() || word.radio.is_empty() || word.esp_hal.is_empty() => {
                problems.push(format!("{address:#010x}: reviewed entry needs radio, esp_hal and a reason"))
            }
            Some(_) => {}
        }
    }
    for address in reviewed.keys().filter(|address| !shared.contains(address)) {
        problems.push(format!(
            "{address:#010x}: reviewed as shared but no longer written by both; remove the entry"
        ));
    }
    if !problems.is_empty() {
        return Err(format!("shared MMIO words of {chip}:\n{}", problems.join("\n")).into());
    }
    let mut by_coordination: BTreeMap<String, usize> = BTreeMap::new();
    for word in &review.word {
        *by_coordination
            .entry(format!("{:?}", word.coordination))
            .or_default() += 1;
    }
    for (coordination, count) in by_coordination {
        eprintln!("shared MMIO words of {chip}: {count} {coordination}");
    }
    Ok(shared.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_words_come_from_write_transactions_only() {
        let svd = r#"<device><peripherals><peripheral><name>P</name><baseAddress>0x1000</baseAddress>
<registers>
<register><name>ROUTE</name><addressOffset>0x0</addressOffset><access>read-write</access></register>
<register><name>CONF</name><addressOffset>0x4</addressOffset></register>
<register><name>CH%s_EVENT</name><addressOffset>0x10</addressOffset><dim>2</dim><dimIncrement>0x8</dimIncrement></register>
</registers></peripheral></peripherals></device>"#;
        let api = r#"
[[field-snapshot-reads]]
peripheral = "P"
register = "ROUTE"
[[field-replace-modifies]]
peripheral = "P"
register = "CONF"
[[fixed-register-sequences]]
peripheral = "P"
steps = [{ register = "CH%s_EVENT", index = 1, value = 3 }]
"#;
        let words = radio_written(svd, api).unwrap();
        // A read-write register only read through the API is not a radio store.
        assert_eq!(
            words.keys().copied().collect::<Vec<_>>(),
            [0x1004, 0x1010, 0x1018]
        );
        assert!(
            radio_written(
                svd,
                "[[zero-register-writes]]\nperipheral = \"P\"\nregister = \"GONE\"\n"
            )
            .is_err()
        );
    }

    #[test]
    fn generated_pac_offsets_and_bases_resolve() {
        let lib = "pub type MODEM_LPCON = crate::Periph<modem_lpcon::RegisterBlock, 0x2010_f000>;";
        assert_eq!(
            pac_peripherals(lib)["MODEM_LPCON"],
            ("modem_lpcon".to_owned(), 0x2010_f000)
        );
        let module = r#"
    #[doc = "0x04 - LP_TIMER_CONF"]
    #[inline(always)]
    pub const fn lp_timer_conf(&self) -> &LP_TIMER_CONF {
    #[doc = "0x100..0x108 - MAP"]
    #[inline(always)]
    pub const fn map(&self, n: usize) -> &MAP {"#;
        let registers = pac_registers(module);
        assert_eq!(registers["lp_timer_conf"], (0x4, 0x8));
        assert_eq!(registers["map"], (0x100, 0x108));
    }

    #[test]
    fn stores_are_found_directly_through_bindings_and_across_lines() {
        let source = r#"
            MODEM_LPCON::regs()
                .clk_conf()
                .modify(|_, w| w.clk_lp_timer_en().bit(on));
            let pmu = crate::peripherals::PMU::regs();
            pmu.imm_modem_icg().write(|w| w);
            INTERRUPT_CORE0::regs().core_0_intr_map(n as usize).write(|w| w);
            HP_SYS_CLKRST::regs().flash_ctrl0().read().bits();
        "#;
        let stores = esp_hal_stores(source);
        let expected: BTreeSet<_> = [
            ("MODEM_LPCON", "clk_conf"),
            ("PMU", "imm_modem_icg"),
            ("INTERRUPT_CORE0", "core_0_intr_map"),
        ]
        .into_iter()
        .map(|(p, r)| (p.to_owned(), r.to_owned()))
        .collect();
        assert_eq!(stores, expected);
    }

    #[test]
    fn other_chips_sources_are_skipped() {
        assert!(applies_to(Path::new("soc/chip-a/clocks.rs"), "chip-a"));
        assert!(applies_to(Path::new("interrupt/mod.rs"), "chip-a"));
        assert!(!applies_to(Path::new("soc/esp32c6/clocks.rs"), "chip-a"));
        assert!(!applies_to(
            Path::new("rtc_cntl/sleep/esp32h2.rs"),
            "chip-a"
        ));
    }
}
