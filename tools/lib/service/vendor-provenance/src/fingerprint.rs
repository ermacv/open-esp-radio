//! Relocation-normalized fingerprints of vendor RV32 functions.
//!
//! Relocatable objects leave every relocated field unresolved, so identical
//! source produces identical bytes except inside those fields. A function's
//! fingerprint covers its code bytes with exactly the bits each relocation
//! patches cleared ([`oer_elf::rv32::mask`], the one relocation table),
//! followed by each relocation's type and, for a target inside the
//! function's own section, its offset from the function start. Opcodes,
//! registers and the relocation shape stay significant.
//!
//! External target names enter only the [`Function::named`] fingerprint, so
//! a function whose code is unchanged but whose obfuscated callees were
//! renamed keeps its [`Function::code`] fingerprint.
//!
//! Instructions are split by the RISC-V length encoding (two bytes unless
//! the low two bits are set). Each gives one token of its masked bytes and
//! relocations; the token sequence is the similarity feature of changed
//! functions ([`similarity_ppm`]).

use oer_elf::{Code, Reference, Site, rv32};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub use oer_elf::is_binary;

/// Similarity prefilters use runs of this many instruction tokens.
const SHINGLE: usize = 4;

/// One defined function of an archive member or ELF.
#[derive(Clone, Debug)]
pub struct Function {
    /// Archive member, or empty for a standalone ELF.
    pub member: String,
    pub name: String,
    /// Code bytes.
    pub size: usize,
    /// Code fingerprint: masked bytes and the name-free relocation shape.
    pub code: String,
    /// Code fingerprint plus external relocation target names.
    pub named: String,
    /// One token per instruction, for similarity.
    pub tokens: Vec<u64>,
    /// Named targets of the relocations that may enter another function
    /// (`call`, `jal`), in body order.
    pub calls: Vec<String>,
}

impl Function {
    /// The fingerprints of one relocatable function.
    pub fn of(code: &Code<'_>) -> Self {
        let mut bytes = code.bytes.to_vec();
        for site in &code.sites {
            rv32::mask(&mut bytes, site.offset, site.r_type);
        }
        let mut by_offset: BTreeMap<usize, Vec<&Site>> = BTreeMap::new();
        for site in &code.sites {
            by_offset.entry(site.offset).or_default().push(site);
        }
        let (mut code_hash, mut named_hash) = (Sha256::new(), Sha256::new());
        let mut tokens = vec![];
        let mut at = 0;
        while at < bytes.len() {
            let width = if bytes[at] & 3 == 3 { 4 } else { 2 }.min(bytes.len() - at);
            let here: Vec<&Site> = by_offset
                .range(at..at + width)
                .flat_map(|(_, list)| list.iter().copied())
                .collect();
            let instruction = &bytes[at..at + width];
            let anonymous = token(instruction, &here, false);
            code_hash.update(anonymous.to_le_bytes());
            named_hash.update(token(instruction, &here, true).to_le_bytes());
            tokens.push(anonymous);
            at += width;
        }
        let calls = code
            .sites
            .iter()
            .filter(|site| rv32::enters(site.r_type))
            .filter_map(|site| match &site.reference {
                Reference::Symbol(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        Self {
            member: code.member.clone(),
            name: code.name.to_owned(),
            size: code.bytes.len(),
            code: hex(code_hash.finalize()),
            named: hex(named_hash.finalize()),
            tokens,
            calls,
        }
    }

    /// Hashed runs of instruction tokens, a cheap similarity prefilter.
    pub fn shingles(&self) -> HashSet<u64> {
        self.tokens
            .windows(SHINGLE)
            .map(|window| {
                window
                    .iter()
                    .fold(0xcbf2_9ce4_8422_2325_u64, |hash, token| {
                        (hash ^ token).wrapping_mul(0x0000_0100_0000_01b3)
                    })
            })
            .collect()
    }
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn token(bytes: &[u8], sites: &[&Site], named: bool) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    for site in sites {
        hasher.update(site.r_type.to_le_bytes());
        match &site.reference {
            Reference::Local(offset) => hasher.update(offset.to_le_bytes()),
            // Every target outside the section is external; only its name,
            // empty for a section or non-symbol target, tells them apart.
            Reference::Symbol(name) if named => hasher.update(name.as_bytes()),
            Reference::Anonymous | Reference::Other if named => {}
            Reference::Symbol(_) | Reference::Anonymous | Reference::Other => {
                hasher.update(b"external")
            }
        }
    }
    u64::from_le_bytes(hasher.finalize()[..8].try_into().expect("eight bytes"))
}

/// Every defined function of an archive's ELF members, or of one ELF.
pub fn functions(bytes: &[u8]) -> oer_elf::Result<Vec<Function>> {
    Ok(functions_and_symbols(bytes)?.0)
}

/// Every defined function and the name of every symbol the members define
/// (sections and files aside), in one pass over an archive's ELF members or
/// one ELF.
pub fn functions_and_symbols(bytes: &[u8]) -> oer_elf::Result<(Vec<Function>, BTreeSet<String>)> {
    let mut functions = vec![];
    let mut symbols = BTreeSet::new();
    for (member, elf) in oer_elf::objects(bytes)? {
        functions.extend(elf.code(&member)?.iter().map(Function::of));
        symbols.extend(
            elf.symbols()
                .filter(|s| {
                    s.defined
                        && !s.name.is_empty()
                        && !matches!(
                            s.kind,
                            oer_elf::SymbolKind::Section | oer_elf::SymbolKind::File
                        )
                })
                .map(|s| s.name.to_owned()),
        );
    }
    Ok((functions, symbols))
}

/// Parts per million of the longest common token subsequence, relative to
/// both lengths (`2 * common / (left + right)`); one million when both are
/// empty.
pub fn similarity_ppm(left: &[u64], right: &[u64]) -> u32 {
    if left.is_empty() && right.is_empty() {
        return 1_000_000;
    }
    let mut previous = vec![0_u32; right.len() + 1];
    let mut current = vec![0_u32; right.len() + 1];
    for token in left {
        for (column, other) in right.iter().enumerate() {
            current[column + 1] = if token == other {
                previous[column] + 1
            } else {
                previous[column + 1].max(current[column])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let common = u64::from(previous[right.len()]);
    let total = (left.len() + right.len()) as u64;
    u32::try_from(2 * common * 1_000_000 / total).unwrap_or(1_000_000)
}

#[cfg(test)]
mod tests;
