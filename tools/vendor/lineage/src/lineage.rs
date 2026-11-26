//! Name recovery composed over an ordered sequence of revisions.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::archive::Revision;
use crate::correspond::{Evidence, Pair, Policy, correlate};

/// Whether `name` carries a generated obfuscation token instead of a source
/// name. A token is a final `_`-separated component, before any compiler
/// suffix such as `.part.0`, of at least sixteen alphanumeric characters with
/// lower case and either a digit or upper case in at least a quarter of its
/// characters. Source identifiers in camel case, such as
/// `onSchedHwListDone`, have neither.
pub fn is_obfuscated(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name);
    let token = base.rsplit('_').next().unwrap_or(base);
    let upper = token.bytes().filter(u8::is_ascii_uppercase).count();
    token.len() >= 16
        && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && token.bytes().any(|byte| byte.is_ascii_lowercase())
        && (token.bytes().any(|byte| byte.is_ascii_digit()) || upper >= token.len().div_ceil(4))
}

/// How generated names are told apart from source names.
#[derive(Clone, Debug, Default)]
pub enum NameClass {
    /// [`is_obfuscated`] decides.
    #[default]
    Heuristic,
    /// A name is generated exactly when it starts with one of these prefixes.
    Prefixes(Vec<String>),
}

impl NameClass {
    /// Whether `name` is a generated name under this rule.
    pub fn is_generated(&self, name: &str) -> bool {
        match self {
            Self::Heuristic => is_obfuscated(name),
            Self::Prefixes(prefixes) => prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix.as_str())),
        }
    }
}

/// Recovered identity of one function of the final revision.
#[derive(Debug, Serialize)]
pub struct Recovered {
    /// Name in the final revision.
    pub name: String,
    /// Archive member in the final revision.
    pub member: String,
    /// Source name, when known.
    pub source_name: Option<String>,
    /// Revision label whose plain name supplied `source_name`.
    pub origin: Option<String>,
    /// Evidence for every revision step since `origin`.
    pub steps: Vec<Step>,
    /// Evidence for every revision step since the earliest revision of the
    /// function's unbroken pairing chain, whether or not it carries a source
    /// name. Its first step names the function in that revision.
    pub chain: Vec<Step>,
}

/// Evidence for one revision step of a lineage.
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub to: String,
    pub previous_name: String,
    pub evidence: Evidence,
    pub identical: bool,
    pub similarity_ppm: u32,
}

/// Counts for one revision step.
#[derive(Debug, Default, Serialize)]
pub struct StepSummary {
    pub from: String,
    pub to: String,
    pub same_name: usize,
    pub exact_body: usize,
    pub call_graph: usize,
    pub similar: usize,
    pub dominant: usize,
    pub neighbourhood: usize,
    pub changed_bodies: usize,
    /// Pairs whose bodies share less than half of their parcels.
    pub dissimilar: usize,
    pub unpaired_left: usize,
    pub unpaired_right: usize,
}

/// Complete lineage report.
#[derive(Debug, Serialize)]
pub struct Lineage {
    pub revisions: Vec<RevisionIdentity>,
    pub steps: Vec<StepSummary>,
    pub functions: Vec<Recovered>,
}

#[derive(Debug, Serialize)]
pub struct RevisionIdentity {
    pub label: String,
    pub sha256: String,
    pub functions: usize,
}

/// Carry source names from the earliest revision to the last one.
pub fn trace(revisions: &[Revision], policy: Policy, class: &NameClass) -> Lineage {
    let Some(first) = revisions.first() else {
        return Lineage {
            revisions: Vec::new(),
            steps: Vec::new(),
            functions: Vec::new(),
        };
    };
    let mut current = seed(first, class);
    let mut steps = Vec::new();
    for [left, right] in revisions.array_windows() {
        let pairs = correlate(left, right, policy);
        let by_left: BTreeMap<usize, &Pair> = pairs.iter().map(|pair| (pair.left, pair)).collect();
        // What every paired function of the older revision brings to its
        // place in the newer one.
        let carried: BTreeMap<usize, (&Recovered, &Pair)> = current
            .iter()
            .enumerate()
            .filter_map(|(place, previous)| {
                let pair = by_left.get(&place)?;
                Some((pair.right, (previous, *pair)))
            })
            .collect();
        let count = |counted: fn(&Pair) -> bool| pairs.iter().filter(|pair| counted(pair)).count();
        let mut next = seed(right, class);
        steps.push(StepSummary {
            from: left.label.clone(),
            to: right.label.clone(),
            same_name: count(|pair| pair.evidence == Evidence::SameName),
            exact_body: count(|pair| pair.evidence == Evidence::ExactBody),
            call_graph: count(|pair| pair.evidence == Evidence::CallGraph),
            similar: count(|pair| matches!(pair.evidence, Evidence::Similar { .. })),
            dominant: count(|pair| matches!(pair.evidence, Evidence::Dominant { .. })),
            neighbourhood: count(|pair| matches!(pair.evidence, Evidence::Neighbourhood { .. })),
            changed_bodies: count(|pair| !pair.identical),
            dissimilar: count(|pair| pair.similarity_ppm < 500_000),
            unpaired_left: (0..current.len())
                .filter(|place| !by_left.contains_key(place))
                .count(),
            unpaired_right: (0..next.len())
                .filter(|place| !carried.contains_key(place))
                .count(),
        });
        for (place, entry) in next.iter_mut().enumerate() {
            // A function without a pair starts its own chain.
            let Some((previous, pair)) = carried.get(&place) else {
                continue;
            };
            entry.chain.clone_from(&previous.chain);
            entry.chain.push(step(right, previous, pair));
            if previous.source_name.is_none() {
                continue;
            }
            // A plain name in the newer revision is authoritative; the
            // lineage only fills generated names.
            if entry.source_name.is_some() && entry.origin.as_deref() == Some(right.label.as_str())
            {
                if entry.source_name == previous.source_name {
                    entry.origin.clone_from(&previous.origin);
                    entry.steps.clone_from(&previous.steps);
                    entry.steps.push(step(right, previous, pair));
                }
                continue;
            }
            entry.source_name.clone_from(&previous.source_name);
            entry.origin.clone_from(&previous.origin);
            entry.steps.clone_from(&previous.steps);
            entry.steps.push(step(right, previous, pair));
        }
        current = next;
    }
    Lineage {
        revisions: revisions
            .iter()
            .map(|revision| RevisionIdentity {
                label: revision.label.clone(),
                sha256: revision.sha256.clone(),
                functions: revision.functions.len(),
            })
            .collect(),
        steps,
        functions: current,
    }
}

fn step(right: &Revision, previous: &Recovered, pair: &Pair) -> Step {
    Step {
        to: right.label.clone(),
        previous_name: previous.name.clone(),
        evidence: pair.evidence,
        identical: pair.identical,
        similarity_ppm: pair.similarity_ppm,
    }
}

fn seed(revision: &Revision, class: &NameClass) -> Vec<Recovered> {
    revision
        .functions
        .iter()
        .map(|function| {
            let plain = !class.is_generated(&function.name);
            Recovered {
                name: function.name.clone(),
                member: function.member.clone(),
                source_name: plain.then(|| function.name.clone()),
                origin: plain.then(|| revision.label.clone()),
                steps: Vec::new(),
                chain: Vec::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::is_obfuscated;

    #[test]
    fn obfuscation_tokens_are_long_mixed_case_components() {
        assert!(is_obfuscated("r_sym_bt_VrTmsQfPlkmys4UL0NZp"));
        assert!(is_obfuscated("r_sym_ble_oDSWrKSM8tZw8SolRxrc.part.0"));
        assert!(!is_obfuscated("r_btdm_sched_insert_with_lock_modify"));
        assert!(!is_obfuscated("r_sched_txn_onSchedHwListDone"));
    }

    #[test]
    fn a_token_without_a_digit_needs_upper_case_in_a_quarter_of_its_characters() {
        // Sixteen characters: four in upper case are a quarter, three are not.
        assert!(is_obfuscated("r_ABCDefghijklmnop"));
        assert!(!is_obfuscated("r_ABCdefghijklmnop"));
        // Seventeen: four are less than a quarter, five are more.
        assert!(!is_obfuscated("r_ABCDefghijklmnopq"));
        assert!(is_obfuscated("r_ABCDEfghijklmnopq"));
        // A digit stands for the upper case; fifteen characters are too few.
        assert!(is_obfuscated("r_abcdefghijklmno1"));
        assert!(!is_obfuscated("r_ABCDEFGHijklmn1"));
        // All in upper case has no lower case.
        assert!(!is_obfuscated("r_ABCDEFGHIJKLMNOP"));
    }
}
