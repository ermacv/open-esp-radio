//! Vendor-use policy for register-accesses schema 9. Expression selection
//! masks are deliberately not evidence of read use.
use super::{DeclaredWord, Function, Region, Target, region_of};
use crate::Result;
use oer_analysis_blobray_domain::{
    ReadPrecision, ReadUses, RegisterMask, RegisterMaskKind, StoredBits, StoredBitsSource,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

mod read_policy;
#[cfg(test)]
mod tests;
mod wire;

use wire::Document;

/// Evidence retained for one access at one candidate address. Masks here
/// refer to bit positions in the containing MMIO word.
#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub function: Function,
    pub symbol: serde_json::Value,
    pub record: u64,
    pub site: u64,
    pub bits: u32,
    pub reason: String,
}

/// All observations of one word, including uncertainty in any observation.
/// Used and unknown masks may overlap across different observations.
#[derive(Debug, Serialize)]
pub struct Word {
    pub address: u32,
    pub region: String,
    pub registers: Vec<String>,
    pub declared_bits: u32,
    pub opaque_bits: u32,
    pub reserved_bits: u32,
    pub reserved_sources: BTreeMap<String, u32>,
    pub loads: usize,
    pub stores: usize,
    pub indexed: usize,
    pub whole_word: bool,
    pub functions: BTreeSet<Function>,
    pub cited: bool,
    pub observed_bits: u32,
    pub used_bits: u32,
    pub exact_bits: u32,
    pub may_bits: u32,
    pub may_only_bits: u32,
    pub diagnostic_bits: u32,
    pub violation_bits: u32,
    pub reserved_violation_bits: u32,
    pub absent_violation_bits: u32,
    pub unknown_bits: u32,
    /// Observed only as preserved, unused, diagnostic or equal to known reset.
    /// These are relative to this corpus; coverage gaps still apply.
    pub inactive_bits: u32,
    pub uses: Vec<Evidence>,
    pub unknown: Vec<Evidence>,
    pub diagnostics: Vec<Evidence>,
}

/// An address the corpus cannot place, or cannot enumerate completely.
#[derive(Debug, Serialize)]
pub struct AddressLimit {
    pub function: Function,
    pub symbol: serde_json::Value,
    pub record: u64,
    pub target: Option<super::Indexed>,
    pub compared_addresses: usize,
}

/// The inputs, assumptions and analysis coverage are copied from Blobray,
/// so a list of violations never implies coverage of an entire vendor.
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: u32,
    pub corpus: wire::Corpus,
    pub words: Vec<Word>,
    pub address_limits: Vec<AddressLimit>,
    pub coverage_gaps: Vec<serde_json::Value>,
    pub resolved_indexed_reads: read_policy::Accounting,
    pub all_reads: read_policy::Accounting,
}

impl Report {
    /// Keep the inventory's cited-function priority without treating a
    /// citation as evidence of any particular bit's use.
    pub fn prioritize(&mut self, cited: &BTreeSet<Function>) {
        for word in &mut self.words {
            word.cited = word
                .functions
                .iter()
                .any(|function| cited.contains(function));
        }
        self.words.sort_by_key(|word| {
            (
                !word.cited,
                word.violation_bits == 0,
                word.unknown_bits == 0,
                std::cmp::Reverse(word.functions.len()),
                word.address,
            )
        });
    }

    pub fn violations(&self) -> usize {
        self.words
            .iter()
            .filter(|word| word.violation_bits != 0)
            .count()
    }

    pub fn render(&self, chip: &str) -> String {
        let summary = &self.corpus.summary;
        let mut text = format!(
            "Vendor bit-use check of {chip}: {} words with used reserved or absent bits\n  {} compared words; {} words with unknown bits\n  functions {} partial {} blocked {} gaps {} unresolved {} indexed {}\n  register-accesses schema {}; ABI {:?}; call_returns {}\n",
            self.violations(),
            self.words.len(),
            self.words
                .iter()
                .filter(|word| word.unknown_bits != 0)
                .count(),
            summary.functions,
            summary.partial_functions,
            summary.blocked_functions,
            summary.gaps,
            summary.unresolved_addresses,
            summary.indexed_addresses,
            self.schema,
            self.corpus.abi,
            self.corpus.call_returns,
        );
        text.push_str(&format!(
            "  reserved-used {} words / {} bits; absent-used {} words / {} bits\n",
            self.words
                .iter()
                .filter(|word| word.reserved_violation_bits != 0)
                .count(),
            self.words
                .iter()
                .map(|word| word.reserved_violation_bits.count_ones())
                .sum::<u32>(),
            self.words
                .iter()
                .filter(|word| word.absent_violation_bits != 0)
                .count(),
            self.words
                .iter()
                .map(|word| word.absent_violation_bits.count_ones())
                .sum::<u32>(),
        ));
        text.push_str("  Corpus: every function of the listed pinned vendor binaries; static may-accesses in owned MMIO ranges.\n  Excludes firmware outputs, non-binary sources, accesses outside these ranges, and RAM layouts.\n  Blocked/gap code and unresolved addresses cannot establish absence; large/unbounded indexed accesses compare only declared arrays/runs.\n  Named diagnostic calls in the cited fixed list do not count as use; all other sinks and Exact/May do. Read-selection does not. Unknown is neither use nor absence.\n");
        self.resolved_indexed_reads
            .render(&mut text, "resolved/indexed MMIO");
        self.all_reads
            .render(&mut text, "all observations including unresolved");
        for input in &self.corpus.inputs {
            text.push_str(&format!("  input {} sha256 {}\n", input.role, input.sha256));
        }
        for range in &summary.ranges {
            text.push_str(&format!("  range {range}\n"));
        }
        text.push_str(&format!("  partial causes: {}\n", summary.partial_causes));
        for word in self.words.iter().filter(|word| word.violation_bits != 0) {
            text.push_str(&format!(
                "\nVIOLATION 0x{:08x} {} [{}] reserved-used 0x{:08x}; absent-used 0x{:08x}; used 0x{:08x} declared 0x{:08x}{}\n",
                word.address, word.region, word.registers.join(", "),
                word.reserved_violation_bits, word.absent_violation_bits, word.used_bits, word.declared_bits, if word.cited { " cited" } else { "" },
            ));
            for (source, mask) in &word.reserved_sources {
                if mask & word.reserved_violation_bits != 0 {
                    text.push_str(&format!(
                        "  RESERVED SOURCE {source} bits {:#010x}\n",
                        mask & word.reserved_violation_bits
                    ));
                }
            }
            for evidence in word
                .uses
                .iter()
                .filter(|e| e.bits & word.violation_bits != 0)
            {
                render_evidence(&mut text, evidence);
            }
        }
        for word in self.words.iter().filter(|word| word.may_only_bits != 0) {
            text.push_str(&format!(
                "\nMAY ONLY 0x{:08x} {} [{}] bits 0x{:08x}\n",
                word.address,
                word.region,
                word.registers.join(", "),
                word.may_only_bits
            ));
        }
        for word in self.words.iter().filter(|word| word.unknown_bits != 0) {
            text.push_str(&format!(
                "\nUNKNOWN 0x{:08x} {} [{}] bits 0x{:08x}{}\n",
                word.address,
                word.region,
                word.registers.join(", "),
                word.unknown_bits,
                if word.cited { " cited" } else { "" },
            ));
            for evidence in &word.unknown {
                render_evidence(&mut text, evidence);
            }
        }
        for limit in &self.address_limits {
            text.push_str(&format!(
                "\nADDRESS LIMIT {}::{} record {} {:?}; {} candidate addresses compared\n  symbol: {}\n",
                limit.function.0,
                limit.function.1,
                limit.record,
                limit.target,
                limit.compared_addresses, limit.symbol,
            ));
        }
        for gap in &self.coverage_gaps {
            text.push_str(&format!("\nCOVERAGE GAP {gap}\n"));
        }
        text
    }
}

fn render_evidence(text: &mut String, evidence: &Evidence) {
    text.push_str(&format!(
        "  {}::{} record {} site {:#x} bits {:#010x}: {}\n    symbol: {}\n",
        evidence.function.0,
        evidence.function.1,
        evidence.record,
        evidence.site,
        evidence.bits,
        evidence.reason,
        evidence.symbol,
    ));
}

/// Compare a complete schema-9 document with the declared source model.
/// A missing load classification is a contract error, never an unused read.
pub fn check(
    chip: &str,
    accesses: &str,
    inputs: &[String],
    regions: &[Region],
    declared: &BTreeMap<u32, DeclaredWord>,
    arrays: &[Vec<u32>],
) -> Result<Report> {
    let policy = read_policy::Policy::for_chip(chip)?;
    if declared
        .values()
        .any(|word| word.reserved.values().any(|mask| mask & word.fields != 0))
    {
        return Err("declared field overlaps cited reserved bits".into());
    }
    let document = Document::parse(accesses, inputs)?;
    let mut words = BTreeMap::<u32, Word>::new();
    let mut address_limits = Vec::new();
    let mut coverage_gaps = Vec::new();
    let mut all_reads = read_policy::Accounting::default();
    let mut resolved_indexed_reads = read_policy::Accounting::default();
    for raw in document.records {
        if raw.get("kind").and_then(|kind| kind.as_str()) != Some("observation") {
            coverage_gaps.push(raw);
            continue;
        }
        let access: wire::Observation = serde_json::from_value(raw)?;
        if access.fact.kind == "expression" {
            continue;
        }
        if access.fact.kind != "memory-access" {
            return Err(format!("unexpected MMIO fact {}", access.fact.kind).into());
        }
        let function = access.function.resolve(inputs)?;
        let width = access.fact.width.ok_or("MMIO access without width")?;
        let full = width_mask(width)?;
        let load = match access.fact.access.as_deref() {
            Some("load" | "float-load") => true,
            Some("store" | "float-store") => false,
            _ => return Err("MMIO access without load/store kind".into()),
        };
        if load {
            let reads = access.reads.as_ref().ok_or("schema-9 load without reads")?;
            validate_reads(reads, full)?;
            all_reads.observe(reads, full, &policy);
            if access.address.is_some() || access.indexed.is_some() {
                resolved_indexed_reads.observe(reads, full, &policy);
            }
        } else {
            validate_stored(access.stored.as_deref(), full)?;
        }
        if let Some(mask) = access.mask
            && mask.bits & !full != 0
        {
            return Err("MMIO mask exceeds access width".into());
        }
        let target = match (access.address, access.indexed) {
            (Some(address), _) => Some(Target::Address(address)),
            (None, Some(indexed)) if indexed.stride != 0 => Some(Target::Indexed(indexed)),
            (None, Some(_)) => return Err("zero indexed stride".into()),
            (None, None) => None,
        };
        let addresses = match target {
            Some(Target::Address(address)) => vec![address],
            Some(Target::Indexed(indexed)) => indexed.addresses(arrays, declared),
            None => vec![],
        };
        if target.is_none()
            || matches!(target, Some(Target::Indexed(i)) if i.count.is_none_or(|n| n > super::MAX_ENUMERATED_INDICES))
        {
            address_limits.push(AddressLimit {
                function: function.clone(),
                symbol: access.function.symbol.clone(),
                record: access.record,
                target: access.indexed,
                compared_addresses: addresses.len(),
            });
        }
        for address in addresses {
            if address.checked_add(u32::from(width) - 1).is_none() {
                return Err("MMIO access wraps address space".into());
            }
            let (reset, known) = access_reset(address, full, declared);
            let evidence = if load {
                read_evidence(access.reads.as_ref().ok_or("load without reads")?, &policy)
            } else {
                store_evidence(access.stored.as_deref(), access.mask, full, reset, known)
            };
            let mut previous_word = None;
            for byte in 0..u32::from(width) {
                let byte_address = address + byte;
                let Some(region) = region_of(regions, byte_address) else {
                    continue;
                };
                let word_address = byte_address & !3;
                let declaration = declared.get(&word_address).cloned().unwrap_or_default();
                let word = words.entry(word_address).or_insert_with(|| Word {
                    address: word_address,
                    region: region.to_owned(),
                    registers: declaration.registers.into_iter().collect(),
                    declared_bits: declaration.fields,
                    opaque_bits: declaration.opaque,
                    reserved_bits: declaration
                        .reserved
                        .values()
                        .fold(0, |mask, bits| mask | bits),
                    reserved_sources: declaration.reserved,
                    loads: 0,
                    stores: 0,
                    indexed: 0,
                    whole_word: false,
                    functions: BTreeSet::new(),
                    cited: false,
                    observed_bits: 0,
                    used_bits: 0,
                    exact_bits: 0,
                    may_bits: 0,
                    may_only_bits: 0,
                    diagnostic_bits: 0,
                    violation_bits: 0,
                    reserved_violation_bits: 0,
                    absent_violation_bits: 0,
                    unknown_bits: 0,
                    inactive_bits: 0,
                    uses: Vec::new(),
                    unknown: Vec::new(),
                    diagnostics: Vec::new(),
                });
                let lane = (byte_address & 3) * 8;
                if previous_word.replace(word_address) != Some(word_address) {
                    word.loads += usize::from(load);
                    word.stores += usize::from(!load);
                    word.indexed += usize::from(matches!(target, Some(Target::Indexed(_))));
                }
                word.whole_word |= width == 4 && address & 3 == 0;
                word.functions.insert(function.clone());
                let project = |bits: u32| ((bits >> (byte * 8)) & 0xff) << lane;
                word.observed_bits |= project(full);
                for (bits, reason, kind) in &evidence {
                    let bits = project(*bits);
                    if bits == 0 {
                        continue;
                    }
                    let entry = Evidence {
                        function: function.clone(),
                        record: access.record,
                        symbol: access.function.symbol.clone(),
                        site: access
                            .fact
                            .offset
                            .ok_or("MMIO access without instruction offset")?,
                        bits,
                        reason: reason.clone(),
                    };
                    match kind {
                        EvidenceKind::Unknown => {
                            word.unknown_bits |= bits;
                            word.unknown.push(entry);
                        }
                        EvidenceKind::Diagnostic => {
                            word.diagnostic_bits |= bits;
                            word.diagnostics.push(entry);
                        }
                        EvidenceKind::Exact | EvidenceKind::May => {
                            word.used_bits |= bits;
                            if matches!(kind, EvidenceKind::Exact) {
                                word.exact_bits |= bits;
                            } else {
                                word.may_bits |= bits;
                            }
                            word.uses.push(entry);
                        }
                    }
                }
            }
        }
    }
    for word in words.values_mut() {
        word.may_only_bits = word.may_bits & !word.exact_bits;
        merge_evidence(&mut word.diagnostics);
        word.reserved_violation_bits = word.used_bits & word.reserved_bits;
        word.absent_violation_bits = word.used_bits & !(word.declared_bits | word.reserved_bits);
        word.violation_bits = word.reserved_violation_bits | word.absent_violation_bits;
        word.inactive_bits = word.observed_bits & !(word.used_bits | word.unknown_bits);
        merge_evidence(&mut word.uses);
        merge_evidence(&mut word.unknown);
    }
    all_reads.finish(&policy);
    resolved_indexed_reads.finish(&policy);
    Ok(Report {
        schema: document.schema,
        corpus: document.corpus,
        words: words.into_values().collect(),
        address_limits,
        coverage_gaps,
        all_reads,
        resolved_indexed_reads,
    })
}

fn merge_evidence(evidence: &mut Vec<Evidence>) {
    let mut merged = BTreeMap::new();
    for entry in evidence.drain(..) {
        let group = merged
            .entry((
                entry.function,
                entry.symbol.to_string(),
                entry.record,
                entry.site,
                entry.reason,
            ))
            .or_insert((0u32, entry.symbol));
        group.0 |= entry.bits;
    }
    *evidence = merged
        .into_iter()
        .map(
            |((function, _, record, site, reason), (bits, symbol))| Evidence {
                function,
                symbol,
                record,
                site,
                bits,
                reason,
            },
        )
        .collect();
}

fn width_mask(width: u8) -> Result<u32> {
    match width {
        1 => Ok(0xff),
        2 => Ok(0xffff),
        4 => Ok(u32::MAX),
        _ => Err(format!("unsupported MMIO access width {width}").into()),
    }
}

fn access_reset(address: u32, full: u32, declared: &BTreeMap<u32, DeclaredWord>) -> (u32, u32) {
    let mut reset = 0;
    let mut known = 0;
    for bit in 0..32 {
        if full & (1 << bit) == 0 {
            continue;
        }
        let byte_address = address + bit / 8;
        let source = 1 << ((byte_address & 3) * 8 + bit % 8);
        if let Some(word) = declared.get(&(byte_address & !3)) {
            if word.reset_known & source != 0 {
                known |= 1 << bit;
            }
            if word.reset_value & source != 0 {
                reset |= 1 << bit;
            }
        }
    }
    (reset, known)
}

fn validate_reads(reads: &ReadUses, full: u32) -> Result<()> {
    let used = reads.uses.iter().fold(0, |bits, item| bits | item.bits);
    let unknown = reads.unknown.iter().fold(0, |bits, item| bits | item.bits);
    let mut covered = 0;
    for class in [used, unknown, reads.preserved, reads.unused] {
        if class & covered != 0 {
            return Err("ReadUses classes overlap".into());
        }
        covered |= class;
    }
    if covered != full {
        return Err("ReadUses does not partition the access width".into());
    }
    Ok(())
}

fn validate_stored(stored: Option<&[StoredBits]>, full: u32) -> Result<()> {
    let Some(stored) = stored else {
        return Ok(());
    };
    let mut covered = 0u32;
    for run in stored {
        if run.width == 0 || u32::from(run.low) + u32::from(run.width) > full.count_ones() {
            return Err("stored run exceeds access width".into());
        }
        let mask = (u32::MAX >> (32 - run.width)) << run.low;
        if covered & mask != 0 {
            return Err("overlapping stored runs".into());
        }
        if let StoredBitsSource::Constant { value } = run.source
            && value & !(u32::MAX >> (32 - run.width)) != 0
        {
            return Err("stored constant exceeds run width".into());
        }
        match run.source {
            StoredBitsSource::Load { width, low, .. }
                if u32::from(low) + u32::from(run.width) > width_mask(width)?.count_ones() =>
            {
                return Err("stored run exceeds source load width".into());
            }
            StoredBitsSource::EntryRegister { low, .. }
                if u32::from(low) + u32::from(run.width) > 32 =>
            {
                return Err("stored run exceeds source register width".into());
            }
            _ => {}
        }
        covered |= mask;
    }
    if covered != full {
        return Err("stored runs do not cover access width".into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum EvidenceKind {
    Exact,
    May,
    Diagnostic,
    Unknown,
}
/// Bits in the accessed value, their source reason and policy classification.
type BitEvidence = (u32, String, EvidenceKind);

fn read_evidence(reads: &ReadUses, policy: &read_policy::Policy) -> Vec<BitEvidence> {
    reads
        .uses
        .iter()
        .map(|used| {
            (
                used.bits,
                format!("read {:?} {:?}", used.precision, used.sink),
                if policy.diagnostic(&used.sink) {
                    EvidenceKind::Diagnostic
                } else if used.precision == ReadPrecision::May {
                    EvidenceKind::May
                } else {
                    EvidenceKind::Exact
                },
            )
        })
        .chain(reads.unknown.iter().map(|unknown| {
            (
                unknown.bits,
                format!("read {:?}", unknown.limit),
                EvidenceKind::Unknown,
            )
        }))
        .collect()
}

fn store_evidence(
    stored: Option<&[StoredBits]>,
    mask: Option<RegisterMask>,
    full: u32,
    reset: u32,
    known: u32,
) -> Vec<BitEvidence> {
    let explicit = mask
        .filter(|mask| mask.kind == RegisterMaskKind::WriteReplacement)
        .map_or(0, |mask| mask.bits);
    let mut evidence = vec![(
        explicit,
        "explicit write-replacement".into(),
        EvidenceKind::Exact,
    )];
    let Some(stored) = stored else {
        evidence.push((
            full & !explicit,
            "store without stored-bit provenance".into(),
            EvidenceKind::Unknown,
        ));
        return evidence;
    };
    for run in stored {
        let bits = ((u32::MAX >> (32 - run.width)) << run.low) & !explicit;
        match run.source {
            StoredBitsSource::Constant { value } => {
                evidence.push((
                    bits & known & ((value << run.low) ^ reset),
                    "write differs from known reset".into(),
                    EvidenceKind::Exact,
                ));
                evidence.push((bits & !known, "unknown reset".into(), EvidenceKind::Unknown));
            }
            StoredBitsSource::Load {
                same_word: true,
                low,
                width,
                ..
            } if low == run.low && width_mask(width).ok() == Some(full) => {}
            _ => evidence.push((
                bits,
                "unresolved stored value".into(),
                EvidenceKind::Unknown,
            )),
        }
    }
    evidence
}
