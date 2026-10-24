//! No MMIO word has two uncoordinated writers.
//!
//! The radio PAC and esp-hal each own registers of one chip. A word that
//! both can store to is a race: a read-modify-write on one side can undo the
//! other's store. This check lists every word a write transaction of the
//! closed radio PAC stores to (`chips/<chip>/registers/policy/api.toml`, located
//! through `chips/<chip>/registers/published/radio.svd`), and every declared
//! word of a typed block, whose claim reaches all of them, that pinned esp-hal also
//! writes (a `PERIPHERAL::regs().register().write/modify` in its sources or
//! in the chip's file of the pinned `esp-metadata-generated`, whose clock and
//! reset writers esp-hal expands; an esp-hal peripheral name such as `SYSTEM`
//! resolves through that file's singleton aliases, `SYSTEM <= PCR`, to the
//! pinned platform PAC and its addresses), and requires each
//! to have a reviewed entry in `chips/<chip>/registers/shared-words.toml` saying how
//! the two writers are coordinated. A reviewed word that is no longer shared
//! is rejected too, so the list stays exact. There is no pending list: a new
//! shared word is fixed or reviewed in the change that introduces it. An
//! entry with `words = N` reviews the run of `N` words from its address, four
//! bytes apart (a source-indexed route array).

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
    /// The radio PAC's writer of the word exists but nothing calls it, so
    /// esp-hal is the only runtime writer; the reason names the owner, and
    /// a change that calls the writer must make one side the only writer.
    UnreachedRadioWriter,
    /// Both sides store whole words (trigger strobes); neither reads,
    /// modifies and writes back, so neither undoes the other.
    WholeWordStores,
    /// The radio only reads the word: its typed view is read-only in the
    /// publication, so no generated writer exists, and no API transaction
    /// stores to it; esp-hal is the only writer. Checked.
    RadioReadsOnly,
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
    /// The words of the run from `address`, four bytes apart; one by default.
    #[serde(default = "one_word")]
    words: u32,
    radio: String,
    esp_hal: String,
    coordination: Coordination,
    reason: String,
}

fn one_word() -> u32 {
    1
}

/// Every reviewed word address, each with its entry; a run expands to all
/// of its words.
fn reviewed_words(review: &[ReviewedWord]) -> Result<BTreeMap<u32, &ReviewedWord>> {
    let mut words = BTreeMap::new();
    for entry in review {
        if entry.words == 0 {
            return Err(format!(
                "{:#010x}: a reviewed run needs at least one word",
                entry.address
            )
            .into());
        }
        for index in 0..entry.words {
            let address = index
                .checked_mul(4)
                .and_then(|offset| entry.address.checked_add(offset))
                .ok_or_else(|| {
                    format!(
                        "{:#010x}: the reviewed run leaves the address space",
                        entry.address
                    )
                })?;
            if words.insert(address, entry).is_some() {
                return Err(format!("{address:#010x}: reviewed twice").into());
            }
        }
    }
    Ok(words)
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
    "field-clear-modifies",
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
    let typed = policy
        .get("options")
        .and_then(|options| options.get("typed-peripherals"))
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    registers.extend(
        typed
            .into_iter()
            .map(|peripheral| (peripheral.to_owned(), TYPED_BLOCK.to_owned())),
    );
    Ok(registers)
}

/// The register name standing for every word of a typed block: the block's
/// claim reaches all of them, so each counts as written.
const TYPED_BLOCK: &str = "*";

/// Words the radio PAC's write transactions store to, with register names.
pub fn radio_written(svd: &str, api: &str) -> Result<BTreeMap<u32, BTreeSet<String>>> {
    let registers = radio_registers(svd)?;
    let mut words: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (peripheral, register) in api_write_registers(api)? {
        if register == TYPED_BLOCK {
            for ((owner, name), addresses) in &registers {
                if *owner == peripheral {
                    for address in addresses {
                        words
                            .entry(*address)
                            .or_default()
                            .insert(format!("{owner}.{name} (typed block)"));
                    }
                }
            }
            continue;
        }
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

/// Words the radio PAC can only read: every radio register at the word is
/// `read-only` in the radio SVD (register, peripheral or device default) and
/// only a typed block's claim, no API write transaction, reaches it.
pub fn radio_read_only(svd: &str, api: &str) -> Result<BTreeSet<u32>> {
    let document = roxmltree::Document::parse(svd)?;
    let device_access = text(document.root_element(), "access");
    let registers = radio_registers(svd)?;
    let mut read_only = BTreeMap::<u32, bool>::new();
    for peripheral in document
        .descendants()
        .filter(|node| node.has_tag_name("peripheral"))
    {
        let peripheral_name = text(peripheral, "name").unwrap_or_default();
        let peripheral_access = text(peripheral, "access").or(device_access);
        for register in peripheral
            .descendants()
            .filter(|node| node.has_tag_name("register"))
        {
            let name = text(register, "name").unwrap_or_default();
            let access = text(register, "access").or(peripheral_access);
            let key = (peripheral_name.to_owned(), name.to_owned());
            for address in registers.get(&key).into_iter().flatten() {
                *read_only.entry(*address).or_insert(true) &= access == Some("read-only");
            }
        }
    }
    let written = radio_written(svd, api)?;
    Ok(read_only
        .into_iter()
        .filter(|(address, read_only)| {
            *read_only
                && written.get(address).is_none_or(|writers| {
                    writers
                        .iter()
                        .all(|writer| writer.ends_with(" (typed block)"))
                })
        })
        .map(|(address, _)| address)
        .collect())
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
/// followed by `pub const fn accessor(`. An array accessor whose body checks
/// `[(); N][n]` and steps `.add(S * n)` spans `N` elements `S` bytes apart
/// from its first offset, whatever its doc range says (svd2rust documents a
/// byte register array by its element size, not its stride).
pub fn pac_registers(module: &str) -> BTreeMap<String, (u32, u32)> {
    let mut registers = BTreeMap::new();
    let mut range = None;
    let mut array: Option<(String, Option<u32>, Option<u32>)> = None;
    for line in module.lines() {
        let line = line.trim();
        if let Some((name, count, stride)) = &mut array {
            if let Some(found) = line
                .strip_prefix("[(); ")
                .and_then(|rest| rest.split_once(']'))
                .and_then(|(number, _)| number.parse().ok())
            {
                *count = Some(found);
            }
            if let Some(found) = line
                .strip_prefix(".add(")
                .and_then(|rest| rest.strip_suffix(" * n)"))
                .and_then(|number| number.parse().ok())
            {
                *stride = Some(found);
            }
            if let (Some(count), Some(stride)) = (*count, *stride) {
                if let Some(entry) = registers.get_mut(name.as_str()) {
                    let (start, _): (u32, u32) = *entry;
                    *entry = (start, start + count * stride);
                }
                array = None;
            }
            if line.starts_with("#[doc") || line.starts_with("pub ") {
                array = None;
            } else {
                continue;
            }
        }
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
                if rest.contains("n: usize") {
                    array = Some((name.to_owned(), None, None));
                }
            }
        } else if !line.starts_with("#[") {
            range = None;
        }
    }
    registers
}

/// The register stores of esp-hal: `write`, `modify`, `write_with_zero`
/// and `reset`.
const STORES: [&str; 4] = ["write(", "modify(", "write_with_zero(", "reset()"];

/// The calls that name a peripheral's register block.
const BLOCKS: [&str; 3] = ["::regs()", "::steal()", "::register_block()"];

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `source` without whitespace, except one space between two identifier
/// characters, so `let mut name` and `X as Y` keep their words apart.
fn compact(source: &str) -> String {
    let mut compact = String::with_capacity(source.len());
    let mut space = false;
    for c in source.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && ident_char(c) && compact.ends_with(ident_char) {
            compact.push(' ');
        }
        space = false;
        compact.push(c);
    }
    compact
}

/// The identifier ending `text`.
fn trailing_ident(text: &str) -> &str {
    trailing(text, ident_char)
}

/// The run of characters `keep` accepts that ends `text`.
fn trailing(text: &str, keep: impl Fn(char) -> bool) -> &str {
    let start = text
        .char_indices()
        .rev()
        .find(|&(_, c)| !keep(c))
        .map_or(0, |(index, c)| index + c.len_utf8());
    &text[start..]
}

/// The identifier starting `text`.
fn leading_ident(text: &str) -> &str {
    let end = text.find(|c: char| !ident_char(c)).unwrap_or(text.len());
    &text[..end]
}

/// Whether `text` ends an expression: nothing, or a separator.
fn ends_value(text: &str) -> bool {
    text.is_empty() || text.starts_with([',', ';', '}', ')'])
}

/// Whether `text`, the text after a register, starts with `.store(`.
fn stores_after(text: &str) -> bool {
    text.strip_prefix('.')
        .is_some_and(|next| STORES.iter().any(|store| next.starts_with(store)))
}

/// `text` with every `<..>` generic list removed.
fn without_generics(text: &str) -> String {
    let mut depth = 0usize;
    text.chars()
        .filter(|&c| match c {
            '<' => {
                depth += 1;
                false
            }
            '>' if depth > 0 => {
                depth -= 1;
                false
            }
            _ => depth == 0,
        })
        .collect()
}

/// The end of the expression starting `text`: its `;`, or the bracket that
/// closes the block around it.
fn expression_end(text: &str) -> usize {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return index,
            ')' | ']' | '}' => depth -= 1,
            ';' if depth == 0 => return index,
            _ => {}
        }
    }
    text.len()
}

/// The text inside the block whose `{` starts `text`.
fn block(text: &str) -> &str {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &text[1..index];
                }
            }
            _ => {}
        }
    }
    text.get(1..).unwrap_or_default()
}

/// What the names of one esp-hal file stand for.
#[derive(Default)]
struct Scope {
    /// Peripheral names behind another name: `use X as Y` and a generic
    /// parameter bound to a trait that peripherals implement through their
    /// `Self::regs()`.
    aliases: BTreeMap<String, BTreeSet<String>>,
    /// Bindings of a peripheral's register block.
    peripherals: BTreeMap<String, BTreeSet<String>>,
    /// Bindings of a register (or cluster) reference: `(peripheral,
    /// register)` pairs.
    registers: BTreeMap<String, BTreeSet<(String, String)>>,
}

impl Scope {
    /// The peripheral names `name` may stand for.
    fn names(&self, name: &str) -> BTreeSet<String> {
        let mut names = self.aliases.get(name).cloned().unwrap_or_default();
        names.insert(name.to_owned());
        names
    }

    /// The peripherals of a block call ending `head`: `X::regs()`,
    /// `X::steal()` or `X::register_block()`, also closed by an `unsafe`
    /// block's `}`.
    fn block_call(&self, head: &str) -> Option<BTreeSet<String>> {
        let head = head.strip_suffix('}').unwrap_or(head);
        BLOCKS.iter().find_map(|call| {
            let name = trailing_constant(head.strip_suffix(call)?);
            (!name.is_empty()).then(|| self.names(name))
        })
    }

    /// The peripherals whose register block the expression ending `head`
    /// is: a block call or a binding of one.
    fn receiver(&self, head: &str) -> BTreeSet<String> {
        self.block_call(head)
            .or_else(|| self.peripherals.get(trailing_ident(head)).cloned())
            .unwrap_or_default()
    }

    /// The register references the value `expression` may be: a register
    /// or cluster call on a register block, ending a value.
    fn register_values(&self, expression: &str) -> BTreeSet<(String, String)> {
        let mut values = BTreeSet::new();
        for (dot, _) in expression.match_indices('.') {
            let peripherals = self.receiver(&expression[..dot]);
            if peripherals.is_empty() {
                continue;
            }
            let Some((register, after)) = call(&expression[dot + 1..]) else {
                continue;
            };
            let register = if ends_value(after) {
                register.to_owned()
            } else if let Some((member, after)) = after.strip_prefix('.').and_then(call)
                && ends_value(after)
            {
                format!("{register}.{member}")
            } else {
                continue;
            };
            values.extend(
                peripherals
                    .into_iter()
                    .map(|peripheral| (peripheral, register.clone())),
            );
        }
        values
    }

    /// The register blocks the value `expression` may be: block calls
    /// ending a value, or another binding.
    fn block_values(&self, expression: &str) -> BTreeSet<String> {
        let mut values = BTreeSet::new();
        for call in BLOCKS {
            for (index, _) in expression.match_indices(call) {
                let after = &expression[index + call.len()..];
                if ends_value(after.strip_prefix('}').unwrap_or(after)) {
                    let name = trailing_constant(&expression[..index]);
                    if !name.is_empty() {
                        values.extend(self.names(name));
                    }
                }
            }
        }
        if let Some(peripherals) = self.peripherals.get(expression) {
            values.extend(peripherals.iter().cloned());
        }
        values
    }
}

/// `use X as Y` aliases of upper-case names in `text`.
fn use_aliases(text: &str, aliases: &mut BTreeMap<String, BTreeSet<String>>) {
    for (index, _) in text.match_indices(" as ") {
        let name = trailing_constant(&text[..index]);
        let alias = leading_ident(&text[index + " as ".len()..]);
        if !name.is_empty()
            && name == trailing_ident(&text[..index])
            && alias == trailing_constant(alias)
            && !alias.is_empty()
        {
            aliases
                .entry(alias.to_owned())
                .or_default()
                .insert(name.to_owned());
        }
    }
}

/// Generic parameters bound to a trait whose implementations in `text`
/// return a peripheral's block (`impl Trait for X { .. Self::regs() .. }`):
/// each stands for every such peripheral.
fn generic_aliases(text: &str, aliases: &mut BTreeMap<String, BTreeSet<String>>) {
    let mut implementors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (index, _) in text.match_indices("impl") {
        if text[..index].ends_with(ident_char) {
            continue;
        }
        let rest = &text[index + "impl".len()..];
        let Some(open) = rest.find('{') else {
            continue;
        };
        let Some((implemented, target)) = rest[..open].split_once(" for ") else {
            continue;
        };
        let target = without_generics(target);
        let target = target.split(" where").next().unwrap_or_default();
        let peripheral = trailing_constant(target);
        let implemented = without_generics(implemented);
        let implemented = trailing_ident(&implemented);
        if !peripheral.is_empty()
            && !implemented.is_empty()
            && block(&rest[open..]).contains("Self::regs()")
        {
            implementors
                .entry(implemented.to_owned())
                .or_default()
                .insert(peripheral.to_owned());
        }
    }
    for (index, _) in text.match_indices(':') {
        let (before, after) = (&text[..index], &text[index + 1..]);
        if before.ends_with(':') || after.starts_with(':') {
            continue;
        }
        let parameter = trailing_constant(before);
        if parameter.is_empty() || parameter != trailing_ident(before) {
            continue;
        }
        if let Some(peripherals) = implementors.get(leading_ident(after)) {
            aliases
                .entry(parameter.to_owned())
                .or_default()
                .extend(peripherals.iter().cloned());
        }
    }
}

/// Every `let name = value;` and every later `name = value;` of a
/// `let name;`, as `(name, value)`.
fn bindings(text: &str) -> Vec<(&str, &str)> {
    let mut bindings = Vec::new();
    let mut declared = BTreeSet::new();
    for (index, _) in text.match_indices("let ") {
        if text[..index].ends_with(ident_char) {
            continue;
        }
        let rest = &text[index + "let ".len()..];
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        let name = leading_ident(rest);
        if name.is_empty() {
            continue;
        }
        let after = &rest[name.len()..];
        if after.starts_with(';') {
            declared.insert(name);
            continue;
        }
        let value = if after.starts_with(':') {
            after.find('=').map(|equals| &after[equals..])
        } else {
            Some(after)
        };
        if let Some(value) = value
            .and_then(|value| value.strip_prefix('='))
            .filter(|value| !value.starts_with(['=', '>']))
        {
            bindings.push((name, &value[..expression_end(value)]));
        }
    }
    for name in declared {
        for (index, _) in text.match_indices(name) {
            let assignment = &text[index + name.len()..];
            if text[..index].ends_with([';', '{', '}'])
                && let Some(value) = assignment
                    .strip_prefix('=')
                    .filter(|value| !value.starts_with(['=', '>']))
            {
                bindings.push((name, &value[..expression_end(value)]));
            }
        }
    }
    bindings
}

/// `(peripheral, register accessor)` pairs that esp-hal source stores to,
/// across line breaks: `PERIPHERAL::regs().register(..).write/modify/reset`
/// and the same through
/// - a `PERIPHERAL::steal()` (also as `unsafe { PERIPHERAL::steal() }`) or
///   a generic `TG::register_block()`, `T::regs()`, where `TG` is bound to
///   a trait whose implementations return `Self::regs()`;
/// - a `use PERIPHERAL as ALIAS`;
/// - a binding of the block (`let regs = ..`), or of the register
///   (`let reg = regs.register(..)`, also deferred, as `let reg; reg =
///   match .. { .. => regs.register(..), .. }`), each value of a
///   `cfg_select!` or `match` counting;
/// - a cluster: `regs.cluster(..).register(..).write`, named
///   `cluster.register`.
pub fn esp_hal_stores(source: &str) -> BTreeSet<(String, String)> {
    let text = compact(source);
    let mut scope = Scope::default();
    use_aliases(&text, &mut scope.aliases);
    generic_aliases(&text, &mut scope.aliases);
    let bindings = bindings(&text);
    // Bindings of bindings resolve in later rounds; the sets only grow.
    loop {
        let mut changed = false;
        for &(name, value) in &bindings {
            for peripheral in scope.block_values(value) {
                changed |= scope
                    .peripherals
                    .entry(name.to_owned())
                    .or_default()
                    .insert(peripheral);
            }
            for register in scope.register_values(value) {
                changed |= scope
                    .registers
                    .entry(name.to_owned())
                    .or_default()
                    .insert(register);
            }
        }
        if !changed {
            break;
        }
    }
    let mut stores = BTreeSet::new();
    for (dot, _) in text.match_indices('.') {
        let (head, rest) = (&text[..dot], &text[dot + 1..]);
        for peripheral in scope.receiver(head) {
            let Some((register, after)) = call(rest) else {
                continue;
            };
            if stores_after(after) {
                stores.insert((peripheral, register.to_owned()));
            } else if let Some((member, after)) = after.strip_prefix('.').and_then(call)
                && stores_after(after)
            {
                stores.insert((peripheral, format!("{register}.{member}")));
            }
        }
        let Some(registers) = scope.registers.get(trailing_ident(head)) else {
            continue;
        };
        if STORES.iter().any(|store| rest.starts_with(store)) {
            stores.extend(registers.iter().cloned());
        } else if let Some((member, after)) = call(rest)
            && stores_after(after)
        {
            stores.extend(registers.iter().map(|(peripheral, register)| {
                (peripheral.clone(), format!("{register}.{member}"))
            }));
        }
    }
    stores
}

/// The upper-case path segment ending `text`, such as `MODEM_LPCON`.
fn trailing_constant(text: &str) -> &str {
    trailing(text, |c| {
        c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'
    })
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

/// The PAC peripheral behind each esp-hal peripheral singleton that
/// `esp-metadata-generated` declares under another name:
/// `peri_type #[doc = ".."] SYSTEM <= PCR() (unstable)`. Virtual singletons,
/// which have no registers, and singletons named as their PAC peripheral are
/// left out.
pub fn singleton_aliases(generated: &str) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for (index, _) in generated.match_indices(" <= ") {
        let name = trailing_constant(generated[..index].trim_end());
        let after = &generated[index + " <= ".len()..];
        let Some((pac, _)) = after.split_once('(') else {
            continue;
        };
        let pac = pac.trim();
        if name.is_empty()
            || pac == name
            || pac.is_empty()
            || !pac
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        aliases.insert(name.to_owned(), pac.to_owned());
    }
    aliases
}

/// The pinned esp-hal sources whose stores count: every file of esp-hal's
/// `src` that `chip` compiles, and the chip's `esp-metadata-generated` file.
pub struct EspHalSources<'a> {
    /// esp-hal's `src` directory.
    pub esp_hal: &'a Path,
    /// `esp-metadata-generated`'s `src` directory.
    pub metadata_generated: &'a Path,
}

/// The platform PAC's register-block modules read so far, by path below
/// its `src`: each module's text and its register ranges.
type Modules = BTreeMap<PathBuf, (String, BTreeMap<String, (u32, u32)>)>;

/// The module at `path` below `pac_src`, read once; empty when absent.
fn module<'m>(
    modules: &'m mut Modules,
    pac_src: &Path,
    path: PathBuf,
) -> &'m (String, BTreeMap<String, (u32, u32)>) {
    modules.entry(path).or_insert_with_key(|path| {
        let text = fs::read_to_string(pac_src.join(path)).unwrap_or_default();
        let registers = pac_registers(&text);
        (text, registers)
    })
}

/// The byte ranges, relative to the block, that a store to `register` of
/// module `name` reaches: the register's range, or for `cluster.member` the
/// member in every element of the cluster (the whole cluster when the
/// member does not resolve).
fn register_ranges(
    modules: &mut Modules,
    pac_src: &Path,
    name: &str,
    register: &str,
) -> Vec<(u32, u32)> {
    let (cluster, member) = match register.split_once('.') {
        Some((cluster, member)) => (cluster, Some(member)),
        None => (register, None),
    };
    let (text, registers) = module(modules, pac_src, PathBuf::from(format!("{name}.rs")));
    let Some(&range) = registers.get(cluster) else {
        return Vec::new();
    };
    let Some(member) = member else {
        return vec![range];
    };
    let elements = cluster_elements(text, cluster, range);
    let (_, members) = module(
        modules,
        pac_src,
        Path::new(name).join(format!("{cluster}.rs")),
    );
    match members.get(member) {
        Some(&(start, end)) => elements
            .into_iter()
            .map(|element| (element + start, element + end))
            .collect(),
        None => vec![range],
    }
}

/// The start offsets of the elements of cluster accessor `name` spanning
/// `range` in a register block's module: `name: [TYPE; N]` in the block's
/// struct, or an accessor stepping `.add(S * n)` over `[(); N]`; a single
/// element otherwise.
pub fn cluster_elements(module: &str, name: &str, (start, end): (u32, u32)) -> Vec<u32> {
    let field = format!("{name}: [");
    let counted = module.lines().find_map(|line| {
        let (_, count) = line.trim().strip_prefix(&field)?.split_once("; ")?;
        count.strip_suffix("],")?.parse::<u32>().ok()
    });
    let accessor = format!("pub const fn {name}(");
    let stepped = module.find(&accessor).and_then(|at| {
        let mut count = None;
        let mut stride = None;
        for line in module[at..].lines().skip(1).map(str::trim) {
            if line.starts_with("pub ") || line.starts_with("#[doc") {
                break;
            }
            count = count.or_else(|| {
                line.strip_prefix("[(); ")?
                    .split_once(']')?
                    .0
                    .parse::<u32>()
                    .ok()
            });
            stride = stride.or_else(|| {
                line.strip_prefix(".add(")?
                    .strip_suffix(" * n)")?
                    .parse::<u32>()
                    .ok()
            });
        }
        count.zip(stride)
    });
    match (counted, stepped) {
        (_, Some((count, stride))) => (0..count).map(|index| start + index * stride).collect(),
        (Some(count), None) if count > 0 => {
            let stride = (end - start) / count;
            (0..count).map(|index| start + index * stride).collect()
        }
        _ => vec![start],
    }
}

/// Every esp-hal store resolved to absolute word addresses.
fn esp_hal_words(
    sources: &EspHalSources<'_>,
    pac_src: &Path,
    chip: &str,
) -> Result<BTreeMap<u32, BTreeSet<String>>> {
    let peripherals = pac_peripherals(&fs::read_to_string(pac_src.join("lib.rs"))?);
    let generated_path = sources
        .metadata_generated
        .join(format!("_generated_{chip}.rs"));
    let generated = fs::read_to_string(&generated_path)
        .map_err(|error| format!("{}: {error}", generated_path.display()))?;
    let aliases = singleton_aliases(&generated);
    let mut modules = Modules::new();
    let mut files = Vec::new();
    rust_files(sources.esp_hal, &mut files)?;
    let mut texts = Vec::new();
    for file in files {
        let relative = file.strip_prefix(sources.esp_hal)?;
        if applies_to(relative, chip) {
            texts.push((relative.display().to_string(), fs::read_to_string(&file)?));
        }
    }
    texts.push((
        format!("esp-metadata-generated/_generated_{chip}.rs"),
        generated,
    ));
    let mut words: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (relative, text) in &texts {
        for (peripheral, register) in esp_hal_stores(text) {
            let pac_name = aliases.get(&peripheral).unwrap_or(&peripheral);
            let Some((module, base)) = peripherals.get(pac_name) else {
                continue;
            };
            for (start, end) in register_ranges(&mut modules, pac_src, module, &register) {
                // A byte register stores into the word that holds it.
                for address in ((base + start) & !3..base + end).step_by(4) {
                    words
                        .entry(address)
                        .or_default()
                        .insert(format!("{peripheral}.{register} ({relative})"));
                }
            }
        }
    }
    Ok(words)
}

/// What is wrong with the reviewed entry of a shared word, if anything.
fn entry_problem(address: u32, word: &ReviewedWord, read_only: &BTreeSet<u32>) -> Option<String> {
    if word.reason.trim().is_empty() || word.radio.is_empty() || word.esp_hal.is_empty() {
        return Some(format!(
            "{address:#010x}: reviewed entry needs radio, esp_hal and a reason"
        ));
    }
    (word.coordination == Coordination::RadioReadsOnly && !read_only.contains(&address)).then(
        || {
            format!(
                "{address:#010x}: reviewed radio-reads-only, but the radio publication can write it"
            )
        },
    )
}

/// Checks `chip`'s shared words against its review file.
pub fn check(
    root: &Path,
    chip: &str,
    esp_hal: &EspHalSources<'_>,
    pac_src: &Path,
) -> Result<usize> {
    let publication = crate::ChipSources::load(
        &root.join(format!("chips/{chip}/registers/publication/registers.toml")),
    )?;
    let svd = fs::read_to_string(&publication.svd)?;
    let api = fs::read_to_string(&publication.api)?;
    let radio = radio_written(&svd, &api)?;
    let read_only = radio_read_only(&svd, &api)?;
    let esp_hal = esp_hal_words(esp_hal, pac_src, chip)?;
    let shared: BTreeSet<u32> = radio
        .keys()
        .filter(|address| esp_hal.contains_key(address))
        .copied()
        .collect();
    let review_path = root.join(format!("chips/{chip}/registers/shared-words.toml"));
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
    let reviewed = reviewed_words(&review.word)?;
    for address in &shared {
        match reviewed.get(address) {
            None => problems.push(format!(
                "{address:#010x}: radio PAC {} and esp-hal {} both write it; make one side the only writer or review its coordination in {}",
                radio[address].iter().cloned().collect::<Vec<_>>().join(", "),
                esp_hal[address].iter().cloned().collect::<Vec<_>>().join(", "),
                review_path.display()
            )),
            Some(word) => problems.extend(entry_problem(*address, word, &read_only)),
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
    for word in reviewed.values() {
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
        let words = radio_written(svd, api).expect("the policy resolves");
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
        // A typed block's claim reaches every word it declares.
        let typed = radio_written(svd, "[options]\ntyped-peripherals = [\"P\"]\n")
            .expect("a typed block resolves");
        assert_eq!(
            typed.keys().copied().collect::<Vec<_>>(),
            [0x1000, 0x1004, 0x1010, 0x1018]
        );
    }

    #[test]
    fn only_a_read_only_view_without_write_transactions_is_radio_read_only() {
        let svd = r#"<device><peripherals>
<peripheral><name>ROUTE</name><baseAddress>0x1000</baseAddress><registers>
<register><name>CORE0</name><addressOffset>0x0</addressOffset><access>read-only</access></register>
<register><name>CORE1</name><addressOffset>0x4</addressOffset><access>read-write</access></register>
</registers></peripheral>
<peripheral><name>VIEW</name><baseAddress>0x2000</baseAddress><access>read-only</access><registers>
<register><name>STATUS</name><addressOffset>0x0</addressOffset></register>
<register><name>CLEAR</name><addressOffset>0x4</addressOffset></register>
</registers></peripheral>
</peripherals></device>"#;
        let typed = "[options]\ntyped-peripherals = [\"ROUTE\", \"VIEW\"]\n";
        // A read-only register, declared or inherited from its peripheral;
        // the read-write route word is writable through its typed block.
        assert_eq!(
            radio_read_only(svd, typed)
                .expect("the policy resolves")
                .into_iter()
                .collect::<Vec<_>>(),
            [0x1000, 0x2000, 0x2004]
        );
        // A write transaction makes the word writable whatever its access.
        let written = format!(
            "{typed}[[fixed-register-images]]\nperipheral = \"VIEW\"\nregister = \"CLEAR\"\n"
        );
        assert_eq!(
            radio_read_only(svd, &written)
                .expect("the policy resolves")
                .into_iter()
                .collect::<Vec<_>>(),
            [0x1000, 0x2000]
        );
    }

    #[test]
    fn a_radio_reads_only_review_of_a_writable_word_fails() {
        let word = |coordination| ReviewedWord {
            address: 0x1000,
            words: 1,
            radio: "ROUTE.CORE0".into(),
            esp_hal: "INTERRUPT_CORE0.core_0_intr_map".into(),
            coordination,
            reason: "esp-hal's interrupt table is the only writer".into(),
        };
        let read_only = BTreeSet::from([0x1000]);
        assert_eq!(
            entry_problem(0x1000, &word(Coordination::RadioReadsOnly), &read_only),
            None
        );
        assert!(
            entry_problem(
                0x1000,
                &word(Coordination::RadioReadsOnly),
                &BTreeSet::new()
            )
            .is_some_and(|problem| problem.contains("radio publication can write it"))
        );
        // Other coordinations are reviewed statements, not checked access.
        assert_eq!(
            entry_problem(
                0x1000,
                &word(Coordination::DisjointLifecycle),
                &BTreeSet::new()
            ),
            None
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
    fn stores_through_a_stolen_peripheral_are_found() {
        let source = r#"
            let clic = unsafe { CLIC::steal() };
            clic.int_ie(cpu_interrupt as usize)
                .write(|w| w.int_ie().set_bit());
            unsafe { CLIC::steal() }.int_config().modify(|_, w| w);
            clic.int_ip(n).read().int_ip().bit_is_set();
        "#;
        let expected: BTreeSet<_> = [("CLIC", "int_ie"), ("CLIC", "int_config")]
            .into_iter()
            .map(|(p, r)| (p.to_owned(), r.to_owned()))
            .collect();
        assert_eq!(esp_hal_stores(source), expected);
        // A store straight on an `unsafe` block's stolen peripheral.
        assert_eq!(
            esp_hal_stores("unsafe { CLIC::steal() }.int_ie(16).write(|w| w.int_ie().set_bit());"),
            pairs(&[("CLIC", "int_ie")])
        );
    }

    fn pairs(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(p, r)| ((*p).to_owned(), (*r).to_owned()))
            .collect()
    }

    #[test]
    fn stores_through_a_register_binding_of_a_cfg_select_block_are_found() {
        // esp-hal 1f646c5 interrupt/software.rs `raise`.
        let source = r#"
            let regs = cfg_select! {
                soc_has_intpri => crate::peripherals::INTPRI::regs(),
                _ => crate::peripherals::SYSTEM::regs(),
            };
            let reg = regs.cpu_intr_from_cpu(NUM as usize);
            reg.write(|w| w.cpu_intr().set_bit());
            _ = reg.read();
        "#;
        assert_eq!(
            esp_hal_stores(source),
            pairs(&[
                ("INTPRI", "cpu_intr_from_cpu"),
                ("SYSTEM", "cpu_intr_from_cpu")
            ])
        );
    }

    #[test]
    fn stores_through_a_deferred_register_binding_are_found() {
        // esp-hal 1f646c5 interrupt/software.rs `reset`.
        let source = r#"
            cfg_select! {
                soc_has_intpri => { let regs = crate::peripherals::INTPRI::regs(); }
                _ => { let regs = crate::peripherals::SYSTEM::regs(); }
            }
            let reg;
            reg = match NUM {
                0 => regs.cpu_intr_from_cpu(0),
                1 => regs.cpu_intr_from_cpu(1),
                _ => unreachable!(),
            };
            reg.write(|w| w.cpu_intr().clear_bit());
        "#;
        assert_eq!(
            esp_hal_stores(source),
            pairs(&[
                ("INTPRI", "cpu_intr_from_cpu"),
                ("SYSTEM", "cpu_intr_from_cpu")
            ])
        );
    }

    #[test]
    fn stores_through_register_and_cluster_bindings_are_found() {
        // esp-hal 1f646c5 timer/systimer.rs `set_period` and `set_target`.
        let source = r#"
            let systimer = SYSTIMER::regs();
            let tconf = systimer.target_conf(self.channel() as usize);
            unsafe { tconf.modify(|_, w| w.period().bits(value)) };
            let comp_load = systimer.comp_load(self.channel() as usize);
            comp_load.write(|w| w.load().set_bit());
            let target = systimer.trgt(self.channel() as usize);
            target.hi().write(|w| w.hi().set((value >> 32) as u32));
            let mode = systimer.target_conf(0).read().period_mode().bit();
        "#;
        assert_eq!(
            esp_hal_stores(source),
            pairs(&[
                ("SYSTIMER", "target_conf"),
                ("SYSTIMER", "comp_load"),
                ("SYSTIMER", "trgt.hi"),
            ])
        );
    }

    #[test]
    fn stores_through_a_generic_register_block_are_found() {
        // esp-hal 1f646c5 timer/timg.rs: `Wdt<TG>` through
        // `TG::register_block()`, which each TIMG implements as `Self::regs()`.
        let source = r#"
            pub trait TimerGroupInstance {
                fn register_block() -> *const RegisterBlock;
            }
            impl TimerGroupInstance for TIMG0<'_> {
                #[inline(always)]
                fn register_block() -> *const RegisterBlock { Self::regs() }
            }
            impl TimerGroupInstance for crate::peripherals::TIMG1<'_> {
                fn register_block() -> *const RegisterBlock { Self::regs() }
            }
            impl<TG> Wdt<TG>
            where
                TG: TimerGroupInstance,
            {
                pub fn feed(&mut self) {
                    let reg_block = unsafe { &*TG::register_block() };
                    reg_block.wdtfeed().write(|w| unsafe { w.bits(1) });
                }
                fn set_write_protection(&mut self, enable: bool) {
                    let reg_block = unsafe { &*TG::register_block() };
                    reg_block
                        .wdtwprotect()
                        .write(|w| unsafe { w.wdt_wkey().bits(wkey) });
                }
            }
        "#;
        let stores = esp_hal_stores(source);
        for peripheral in ["TIMG0", "TIMG1"] {
            for register in ["wdtfeed", "wdtwprotect"] {
                assert!(
                    stores.contains(&(peripheral.to_owned(), register.to_owned())),
                    "{peripheral}.{register}: {stores:?}"
                );
            }
        }
    }

    #[test]
    fn stores_through_cfg_select_register_values_and_use_aliases_are_found() {
        // esp-hal 1f646c5 rtc_cntl/mod.rs: the RWDT's key and configuration.
        let source = r#"
            cfg_select! {
                esp32 => { use crate::peripherals::{LPWR, LPWR as LP_WDT}; }
                _ => { use crate::peripherals::LP_WDT; }
            }
            fn set_write_protection(&mut self, enable: bool) {
                let reg = cfg_select! {
                    esp32p4 => LP_WDT::regs().wprotect(),
                    _ => LP_WDT::regs().wdtwprotect(),
                };
                reg.write(|w| unsafe { w.bits(wkey) });
            }
            fn set_enabled(&mut self, enable: bool) {
                let regs = LP_WDT::regs();
                let config0 = cfg_select! {
                    esp32p4 => regs.config0(),
                    _ => regs.wdtconfig0(),
                };
                config0.modify(|_, w| w.wdt_en().bit(enable));
            }
        "#;
        let mut expected = BTreeSet::new();
        for peripheral in ["LP_WDT", "LPWR"] {
            for register in ["wprotect", "wdtwprotect", "config0", "wdtconfig0"] {
                expected.insert((peripheral.to_owned(), register.to_owned()));
            }
        }
        assert_eq!(esp_hal_stores(source), expected);
    }

    #[test]
    fn stores_to_a_clustered_register_are_found() {
        // esp-hal 1f646c5 soc/esp32c5/clocks.rs and the C5's
        // esp-metadata-generated: PCR's TIMERGROUP clusters.
        let source = r#"
            PCR::regs()
                .timergroup(n)
                .wdt_clk_conf()
                .modify(|_, w| w.wdt_clk_en().bit(en));
            crate::peripherals::SYSTEM::regs()
                .timergroup(1)
                .conf()
                .modify(|_, w| w.tg_clk_en().bit(enable));
            PCR::regs().timergroup(0).conf().read().tg_rst_en().bit();
        "#;
        assert_eq!(
            esp_hal_stores(source),
            pairs(&[
                ("PCR", "timergroup.wdt_clk_conf"),
                ("SYSTEM", "timergroup.conf")
            ])
        );
    }

    #[test]
    fn a_comment_does_not_join_the_next_receiver() {
        // esp-hal 1f646c5 soc/esp32c5/clocks.rs: a comment ending in capitals
        // right before the receiver, and text that is not ASCII.
        let source = "w.x().set_bit() });\n// BBPLL CALIBRATION START – go\nI2C_ANA_MST::regs().ana_conf0().modify(|_, w| w);";
        assert_eq!(
            esp_hal_stores(source),
            pairs(&[("I2C_ANA_MST", "ana_conf0")])
        );
    }

    #[test]
    fn a_cluster_store_reaches_its_member_in_every_element() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let block = r#"
pub struct RegisterBlock {
    _reserved0: [u8; 0x54],
    timergroup: [TIMERGROUP; 2],
}
impl RegisterBlock {
    #[doc = "0x54..0x6c - Cluster TIMERGROUP%s"]
    #[inline(always)]
    pub const fn timergroup(&self, n: usize) -> &TIMERGROUP {
        &self.timergroup[n]
    }
}"#;
        let cluster = r#"
    #[doc = "0x00 - TIMERGROUP0 configuration register"]
    #[inline(always)]
    pub const fn conf(&self) -> &CONF {
        &self.conf
    }
    #[doc = "0x08 - TIMERGROUP0_WDT_CLK configuration register"]
    #[inline(always)]
    pub const fn wdt_clk_conf(&self) -> &WDT_CLK_CONF {
        &self.wdt_clk_conf
    }"#;
        fs::create_dir(directory.path().join("pcr")).expect("the cluster directory");
        fs::write(directory.path().join("pcr.rs"), block).expect("the block module");
        fs::write(directory.path().join("pcr/timergroup.rs"), cluster).expect("the cluster module");
        let mut modules = Modules::new();
        let ranges =
            |register| register_ranges(&mut Modules::new(), directory.path(), "pcr", register);
        assert_eq!(
            ranges("timergroup.wdt_clk_conf"),
            [(0x5c, 0x60), (0x68, 0x6c)]
        );
        assert_eq!(ranges("timergroup.conf"), [(0x54, 0x58), (0x60, 0x64)]);
        // An unknown member reaches the whole cluster.
        assert_eq!(ranges("timergroup.other"), [(0x54, 0x6c)]);
        assert_eq!(
            register_ranges(&mut modules, directory.path(), "pcr", "timergroup"),
            [(0x54, 0x6c)]
        );
    }

    #[test]
    fn a_stepped_cluster_array_has_its_count_and_stride() {
        let module = r#"
    #[doc = "0x1c..0x34 - Cluster TRGT%s"]
    #[inline(always)]
    pub const fn trgt(&self, n: usize) -> &TRGT {
        #[allow(clippy::no_effect)]
        [(); 3][n];
        unsafe {
            &*core::ptr::from_ref(self)
                .cast::<u8>()
                .add(28)
                .add(8 * n)
                .cast()
        }
    }"#;
        assert_eq!(
            cluster_elements(module, "trgt", (0x1c, 0x34)),
            [0x1c, 0x24, 0x2c]
        );
        assert_eq!(cluster_elements(module, "other", (0x40, 0x48)), [0x40]);
    }

    #[test]
    fn an_array_spans_its_element_count_at_its_stride() {
        // svd2rust documents a byte array by element size: 48 one-byte
        // elements four bytes apart from 0x1001 end at 0x10c1, not 0x1031.
        let module = r#"
    #[doc = "0x1001..0x1031 - Interrupt enable register."]
    #[inline(always)]
    pub const fn int_ie(&self, n: usize) -> &INT_IE {
        #[allow(clippy::no_effect)]
        [(); 48][n];
        unsafe {
            &*core::ptr::from_ref(self)
                .cast::<u8>()
                .add(4096)
                .add(4 * n)
                .cast()
        }
    }
    #[doc = "0x00 - INT_CONFIG"]
    #[inline(always)]
    pub const fn int_config(&self) -> &INT_CONFIG {"#;
        let registers = pac_registers(module);
        assert_eq!(registers["int_ie"], (0x1001, 0x10c1));
        assert_eq!(registers["int_config"], (0x0, 0x4));
    }

    #[test]
    fn a_reviewed_run_covers_its_words_four_bytes_apart() {
        let review: Review = toml_edit::de::from_str(
            r#"schema = 1
[[word]]
address = 0x1000
words = 3
radio = "R.MAP"
esp_hal = "P.map"
coordination = "shared-lock"
reason = "test"
[[word]]
address = 0x2000
radio = "R.ONE"
esp_hal = "P.one"
coordination = "whole-word-stores"
reason = "test"
"#,
        )
        .expect("a review");
        let words = reviewed_words(&review.word).expect("disjoint runs");
        assert_eq!(
            words.keys().copied().collect::<Vec<_>>(),
            [0x1000, 0x1004, 0x1008, 0x2000]
        );
        let overlapping: Review = toml_edit::de::from_str(
            r#"schema = 1
[[word]]
address = 0x1000
words = 2
radio = "R"
esp_hal = "P"
coordination = "shared-lock"
reason = "test"
[[word]]
address = 0x1004
radio = "R"
esp_hal = "P"
coordination = "shared-lock"
reason = "test"
"#,
        )
        .expect("a review");
        assert!(reviewed_words(&overlapping.word).is_err());
    }

    #[test]
    fn singleton_aliases_name_the_pac_peripheral() {
        let generated = r#"
            _for_each_inner_peripheral!((@ peri_type #[doc =
            "SYSTEM peripheral singleton"] SYSTEM <= PCR() (unstable)));
            _for_each_inner_peripheral!((@ peri_type #[doc = "UART0 peripheral singleton"] UART0 <= UART0(UART0 : {}) (unstable)));
            _for_each_inner_peripheral!((@ peri_type #[doc = "ADC1 peripheral singleton"] ADC1 <= virtual() (unstable)));
        "#;
        let aliases = singleton_aliases(generated);
        assert_eq!(
            aliases.into_iter().collect::<Vec<_>>(),
            [("SYSTEM".to_owned(), "PCR".to_owned())]
        );
    }

    #[test]
    fn a_generated_writer_through_an_alias_is_found() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let esp_hal = directory.path().join("esp-hal");
        let generated = directory.path().join("generated");
        let pac = directory.path().join("pac");
        for path in [&esp_hal, &generated, &pac] {
            fs::create_dir(path).expect("a source directory");
        }
        fs::write(
            esp_hal.join("lib.rs"),
            "PCR::regs().uart0_conf().write(|w| w);",
        )
        .expect("an esp-hal source");
        fs::write(
            generated.join("_generated_chip-a.rs"),
            r#"(@ peri_type #[doc = "SYSTEM peripheral singleton"] SYSTEM <= PCR() (unstable))
            Peripheral::Saradc => {
                crate::peripherals::SYSTEM::regs()
                    .saradc_conf()
                    .modify(|_, w| w.saradc_reg_clk_en().bit(enable));
            }"#,
        )
        .expect("a generated source");
        // Another chip's generated file is not read.
        fs::write(
            generated.join("_generated_chip-b.rs"),
            "PCR::regs().other_conf().write(|w| w);",
        )
        .expect("another chip's generated source");
        fs::write(
            pac.join("lib.rs"),
            "pub type PCR = crate::Periph<pcr::RegisterBlock, 0x6009_6000>;",
        )
        .expect("a PAC root");
        fs::write(
            pac.join("pcr.rs"),
            r#"
    #[doc = "0x00 - UART0_CONF"]
    pub const fn uart0_conf(&self) -> &UART0_CONF {
    #[doc = "0x08 - OTHER_CONF"]
    pub const fn other_conf(&self) -> &OTHER_CONF {
    #[doc = "0x88 - SARADC_CONF"]
    pub const fn saradc_conf(&self) -> &SARADC_CONF {"#,
        )
        .expect("a PAC module");
        let words = esp_hal_words(
            &EspHalSources {
                esp_hal: &esp_hal,
                metadata_generated: &generated,
            },
            &pac,
            "chip-a",
        )
        .expect("the sources resolve");
        assert_eq!(
            words.keys().copied().collect::<Vec<_>>(),
            [0x6009_6000, 0x6009_6088]
        );
        assert_eq!(
            words[&0x6009_6088].iter().collect::<Vec<_>>(),
            ["SYSTEM.saradc_conf (esp-metadata-generated/_generated_chip-a.rs)"]
        );
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
