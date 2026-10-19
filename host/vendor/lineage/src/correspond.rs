//! Function correspondence between two archive revisions.
//!
//! Evidence is applied from strongest to weakest, and each step only pairs
//! functions that are still unpaired on both sides:
//!
//! 1. the same symbol name, unique in both revisions;
//! 2. the same normalized body, unique among the unpaired functions;
//! 3. the call graph: paired callers vote for the callees at corresponding
//!    call positions, until no vote changes. Call lists of equal length
//!    correspond position by position; lists of different length are aligned
//!    on their already corresponding callees, and only the positions inside
//!    equally long gaps between those anchors correspond. A voted pair must
//!    still share a minimum part of its body;
//! 4. body similarity, accepted only as a mutual best: either above the
//!    similarity minimum with a required margin over the second candidate, or
//!    above a lower dominance floor while at least a fixed multiple of the
//!    second candidate in both directions;
//! 5. the neighbourhood: the functions a paired caller's counterpart calls,
//!    and the callers of a paired callee's counterpart, are where an unpaired
//!    function is expected. Each such relation supports a candidate; the
//!    candidate with the most support, then the most similar body, is
//!    accepted only as a unique and mutual best above a similarity floor.
//!
//! Steps 2 to 5 repeat until none pairs another function: every accepted
//! pair leaves the candidate pools, so a later pass can find a function's
//! counterpart once a closer rival has been paired elsewhere.
//!
//! Ambiguity is never resolved by choice: an unpaired function stays unpaired.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;

use crate::archive::Revision;
use oer_vendor_provenance::fingerprint::similarity_ppm;

/// Candidates kept after the shingle prefilter.
const PREFILTER_CANDIDATES: usize = 6;

/// How a pair was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Evidence {
    SameName,
    ExactBody,
    CallGraph,
    Similar {
        ppm: u32,
    },
    /// A mutual best below the similarity minimum that dominates its runner
    /// up in both directions.
    Dominant {
        ppm: u32,
        runner_up_ppm: u32,
    },
    /// The unique mutual best among the functions paired callers and callees
    /// expect, with the number of supporting relations.
    Neighbourhood {
        support: u32,
        ppm: u32,
    },
}

/// One function pair between the left and right revisions.
#[derive(Clone, Copy, Debug)]
pub struct Pair {
    pub left: usize,
    pub right: usize,
    pub evidence: Evidence,
    /// Whether the normalized bodies are identical.
    pub identical: bool,
    /// Body similarity in parts per million; one million when identical.
    pub similarity_ppm: u32,
}

/// Similarity acceptance policy.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Minimum similarity in parts per million.
    pub minimum_ppm: u32,
    /// Minimum lead over the next candidate in parts per million.
    pub margin_ppm: u32,
    /// Minimum body similarity of a call-graph pair in parts per million.
    pub call_graph_minimum_ppm: u32,
    /// Minimum similarity of a dominant pair in parts per million.
    pub dominant_minimum_ppm: u32,
    /// How many times a dominant pair must exceed its runner up.
    pub dominance_ratio: u32,
    /// Minimum similarity of a neighbourhood pair in parts per million.
    pub neighbourhood_minimum_ppm: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum_ppm: 850_000,
            margin_ppm: 50_000,
            call_graph_minimum_ppm: 500_000,
            dominant_minimum_ppm: 500_000,
            dominance_ratio: 2,
            neighbourhood_minimum_ppm: 600_000,
        }
    }
}

/// Pair the functions of `left` and `right`.
pub fn correlate(left: &Revision, right: &Revision, policy: Policy) -> Vec<Pair> {
    let mut state = State::new(left, right);
    state.pair_same_names();
    loop {
        let paired = state.pairs.len();
        state.pair_exact_bodies();
        state.pair_call_graph(policy);
        state.pair_similar(policy);
        state.pair_call_graph(policy);
        state.pair_neighbourhood(policy);
        if state.pairs.len() == paired {
            break;
        }
    }
    let mut pairs: Vec<Pair> = state.pairs.into_values().collect();
    pairs.sort_by_key(|pair| pair.right);
    pairs
}

struct State<'a> {
    left: &'a Revision,
    right: &'a Revision,
    left_names: HashMap<&'a str, usize>,
    right_names: HashMap<&'a str, usize>,
    /// Pairs keyed by the left function.
    pairs: BTreeMap<usize, Pair>,
    right_paired: HashSet<usize>,
}

impl<'a> State<'a> {
    fn new(left: &'a Revision, right: &'a Revision) -> Self {
        Self {
            left,
            right,
            left_names: unique_names(left),
            right_names: unique_names(right),
            pairs: BTreeMap::new(),
            right_paired: HashSet::new(),
        }
    }

    fn insert(&mut self, left: usize, right: usize, evidence: Evidence) {
        let (left_body, right_body) = (&self.left.functions[left], &self.right.functions[right]);
        let identical = left_body.code == right_body.code;
        let similarity_ppm = match evidence {
            Evidence::Similar { ppm }
            | Evidence::Dominant { ppm, .. }
            | Evidence::Neighbourhood { ppm, .. } => ppm,
            _ if identical => 1_000_000,
            _ => similarity_ppm(&left_body.tokens, &right_body.tokens),
        };
        self.pairs.insert(
            left,
            Pair {
                left,
                right,
                evidence,
                identical,
                similarity_ppm,
            },
        );
        self.right_paired.insert(right);
    }

    fn unpaired_left(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.left.functions.len()).filter(|index| !self.pairs.contains_key(index))
    }

    fn unpaired_right(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.right.functions.len()).filter(|index| !self.right_paired.contains(index))
    }

    fn pair_same_names(&mut self) {
        let matches: Vec<(usize, usize)> = self
            .left_names
            .iter()
            .filter_map(|(name, left)| self.right_names.get(name).map(|right| (*left, *right)))
            .collect();
        for (left, right) in matches {
            self.insert(left, right, Evidence::SameName);
        }
    }

    fn pair_exact_bodies(&mut self) {
        let mut left_bodies: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in self.unpaired_left() {
            left_bodies
                .entry(self.left.functions[index].code.as_str())
                .or_default()
                .push(index);
        }
        let mut right_bodies: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in self.unpaired_right() {
            right_bodies
                .entry(self.right.functions[index].code.as_str())
                .or_default()
                .push(index);
        }
        for (fingerprint, lefts) in left_bodies {
            if let (&[left], Some(&[right])) = (
                lefts.as_slice(),
                right_bodies.get(fingerprint).map(Vec::as_slice),
            ) {
                self.insert(left, right, Evidence::ExactBody);
            }
        }
    }

    fn pair_call_graph(&mut self, policy: Policy) {
        // Rejected pairs are not voted for again.
        let mut rejected: BTreeSet<(usize, usize)> = BTreeSet::new();
        loop {
            let mut votes: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            let mut voters: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            for pair in self.pairs.values() {
                let left_calls = &self.left.functions[pair.left].calls;
                let right_calls = &self.right.functions[pair.right].calls;
                for (left_position, right_position) in
                    self.corresponding_calls(left_calls, right_calls)
                {
                    let (left_callee, right_callee) =
                        (&left_calls[left_position], &right_calls[right_position]);
                    let (Some(&left), Some(&right)) = (
                        self.left_names.get(left_callee.as_str()),
                        self.right_names.get(right_callee.as_str()),
                    ) else {
                        continue;
                    };
                    if self.pairs.contains_key(&left)
                        || self.right_paired.contains(&right)
                        || rejected.contains(&(left, right))
                    {
                        continue;
                    }
                    votes.entry(left).or_default().insert(right);
                    voters.entry(right).or_default().insert(left);
                }
            }
            let accepted: Vec<(usize, usize)> = votes
                .iter()
                .filter_map(|(left, rights)| {
                    let right = single(rights)?;
                    (single(&voters[&right]) == Some(*left)).then_some((*left, right))
                })
                .collect();
            let mut progressed = false;
            for (left, right) in accepted {
                let score = similarity_ppm(
                    &self.left.functions[left].tokens,
                    &self.right.functions[right].tokens,
                );
                if score < policy.call_graph_minimum_ppm {
                    rejected.insert((left, right));
                } else {
                    self.insert(left, right, Evidence::CallGraph);
                }
                progressed = true;
            }
            if !progressed {
                return;
            }
        }
    }

    /// Whether two callees already correspond: the same name, or a pair.
    fn anchored(&self, left: &str, right: &str) -> bool {
        left == right
            || self
                .left_names
                .get(left)
                .and_then(|index| self.pairs.get(index))
                .is_some_and(|pair| self.right_names.get(right) == Some(&pair.right))
    }

    /// Call positions that correspond between two call lists: every position
    /// of equally long lists, otherwise the positions inside equally long gaps
    /// between the anchors of a longest common subsequence of corresponding
    /// callees.
    fn corresponding_calls(&self, left: &[String], right: &[String]) -> Vec<(usize, usize)> {
        if left.len() == right.len() {
            return (0..left.len()).map(|index| (index, index)).collect();
        }
        let mut lengths = vec![vec![0_u32; right.len() + 1]; left.len() + 1];
        for (i, left_callee) in left.iter().enumerate().rev() {
            for (j, right_callee) in right.iter().enumerate().rev() {
                lengths[i][j] = if self.anchored(left_callee, right_callee) {
                    lengths[i + 1][j + 1] + 1
                } else {
                    lengths[i + 1][j].max(lengths[i][j + 1])
                };
            }
        }
        let mut anchors = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < left.len() && j < right.len() {
            if self.anchored(&left[i], &right[j]) && lengths[i][j] == lengths[i + 1][j + 1] + 1 {
                anchors.push((i, j));
                i += 1;
                j += 1;
            } else if lengths[i + 1][j] >= lengths[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        anchors.push((left.len(), right.len()));
        let mut positions = Vec::new();
        let (mut left_start, mut right_start) = (0, 0);
        for (left_end, right_end) in anchors {
            if left_end - left_start == right_end - right_start {
                positions.extend((left_start..left_end).zip(right_start..right_end));
            }
            (left_start, right_start) = (left_end + 1, right_end + 1);
        }
        positions
    }

    fn pair_neighbourhood(&mut self, policy: Policy) {
        let left = Calls::new(self.left, &self.left_names);
        let right = Calls::new(self.right, &self.right_names);
        let left_to_right: HashMap<usize, usize> = self
            .pairs
            .values()
            .map(|pair| (pair.left, pair.right))
            .collect();
        let right_to_left: HashMap<usize, usize> = self
            .pairs
            .values()
            .map(|pair| (pair.right, pair.left))
            .collect();
        let left_paired: HashSet<usize> = self.pairs.keys().copied().collect();
        let accepted: Vec<(usize, usize, Evidence)> = self
            .unpaired_left()
            .filter_map(|left_index| {
                let candidates = expected(
                    left_index,
                    &left,
                    &right,
                    &left_to_right,
                    &self.right_paired,
                );
                let (right_index, support, ppm) =
                    best_expected(left_index, candidates, self.left, self.right, policy)?;
                let candidates = expected(right_index, &right, &left, &right_to_left, &left_paired);
                let (back, _, _) =
                    best_expected(right_index, candidates, self.right, self.left, policy)?;
                (back == left_index).then_some((
                    left_index,
                    right_index,
                    Evidence::Neighbourhood { support, ppm },
                ))
            })
            .collect();
        for (left_index, right_index, evidence) in accepted {
            self.insert(left_index, right_index, evidence);
        }
    }

    fn pair_similar(&mut self, policy: Policy) {
        let lefts: Vec<usize> = self.unpaired_left().collect();
        let rights: Vec<usize> = self.unpaired_right().collect();
        let right_shingles: Vec<HashSet<u64>> = rights
            .iter()
            .map(|index| self.right.functions[*index].shingles())
            .collect();
        // Best and second-best score per function, in both directions.
        let mut best_right: HashMap<usize, (usize, u32, u32)> = HashMap::new();
        let mut best_left: HashMap<usize, (usize, u32, u32)> = HashMap::new();
        for left in lefts {
            let body = &self.left.functions[left];
            let shingles = body.shingles();
            let mut candidates: Vec<(u64, usize)> = rights
                .iter()
                .zip(&right_shingles)
                .filter(|(right, _)| comparable(body.size, self.right.functions[**right].size))
                .map(|(right, other)| (jaccard_ppm(&shingles, other), *right))
                .collect();
            candidates.sort_by(|a, b| b.cmp(a));
            candidates.truncate(PREFILTER_CANDIDATES);
            for (_, right) in candidates {
                let score = similarity_ppm(&body.tokens, &self.right.functions[right].tokens);
                record(&mut best_right, left, right, score);
                record(&mut best_left, right, left, score);
            }
        }
        let accepted: Vec<(usize, usize, Evidence)> = best_right
            .iter()
            .filter_map(|(left, (right, score, second))| {
                let (back, _, back_second) = best_left.get(right)?;
                if *back != *left {
                    return None;
                }
                let lead = |second: u32| score.saturating_sub(second) >= policy.margin_ppm;
                let dominates = |second: u32| {
                    u64::from(*score) >= u64::from(second) * u64::from(policy.dominance_ratio)
                };
                if *score >= policy.minimum_ppm && lead(*second) && lead(*back_second) {
                    Some((*left, *right, Evidence::Similar { ppm: *score }))
                } else if *score >= policy.dominant_minimum_ppm
                    && dominates(*second)
                    && dominates(*back_second)
                {
                    let runner_up_ppm = (*second).max(*back_second);
                    Some((
                        *left,
                        *right,
                        Evidence::Dominant {
                            ppm: *score,
                            runner_up_ppm,
                        },
                    ))
                } else {
                    None
                }
            })
            .collect();
        for (left, right, evidence) in accepted {
            self.insert(left, right, evidence);
        }
    }
}

fn unique_names(revision: &Revision) -> HashMap<&str, usize> {
    let mut seen: HashMap<&str, Option<usize>> = HashMap::new();
    for (index, function) in revision.functions.iter().enumerate() {
        seen.entry(function.name.as_str())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(index));
    }
    seen.into_iter()
        .filter_map(|(name, index)| index.map(|index| (name, index)))
        .collect()
}

fn single(set: &BTreeSet<usize>) -> Option<usize> {
    let mut iter = set.iter();
    match (iter.next(), iter.next()) {
        (Some(value), None) => Some(*value),
        _ => None,
    }
}

fn comparable(left: usize, right: usize) -> bool {
    let (small, large) = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    large <= small * 2 + 16
}

fn jaccard_ppm(left: &HashSet<u64>, right: &HashSet<u64>) -> u64 {
    let union = left.union(right).count() as u64;
    if union == 0 {
        return 0;
    }
    left.intersection(right).count() as u64 * 1_000_000 / union
}

/// Keep the best candidate and the best score among the others.
fn record(table: &mut HashMap<usize, (usize, u32, u32)>, key: usize, candidate: usize, score: u32) {
    let entry = table.entry(key).or_insert((candidate, score, 0));
    if entry.0 == candidate {
        entry.1 = entry.1.max(score);
    } else if score > entry.1 {
        *entry = (candidate, score, entry.1);
    } else {
        entry.2 = entry.2.max(score);
    }
}

/// Call relations between the uniquely named functions of one revision.
struct Calls {
    callees: Vec<BTreeSet<usize>>,
    callers: Vec<BTreeSet<usize>>,
}

impl Calls {
    fn new(revision: &Revision, names: &HashMap<&str, usize>) -> Self {
        let count = revision.functions.len();
        let mut callees = vec![BTreeSet::new(); count];
        let mut callers = vec![BTreeSet::new(); count];
        for (caller, function) in revision.functions.iter().enumerate() {
            for callee in &function.calls {
                if let Some(&callee) = names.get(callee.as_str()) {
                    callees[caller].insert(callee);
                    callers[callee].insert(caller);
                }
            }
        }
        Self { callees, callers }
    }
}

/// Unpaired functions of the other revision where `index` is expected, with
/// the number of relations that support each: the callees of each paired
/// caller's counterpart and the callers of each paired callee's counterpart.
fn expected(
    index: usize,
    this: &Calls,
    other: &Calls,
    counterpart: &HashMap<usize, usize>,
    other_paired: &HashSet<usize>,
) -> BTreeMap<usize, u32> {
    let mut support: BTreeMap<usize, u32> = BTreeMap::new();
    let related = [
        (&this.callers[index], &other.callees),
        (&this.callees[index], &other.callers),
    ];
    for (neighbours, relation) in related {
        for neighbour in neighbours {
            let Some(&paired) = counterpart.get(neighbour) else {
                continue;
            };
            for candidate in &relation[paired] {
                if !other_paired.contains(candidate) {
                    *support.entry(*candidate).or_default() += 1;
                }
            }
        }
    }
    support
}

/// The unique best expected candidate above the similarity floor, as
/// (candidate, support, similarity): the most support, then the most similar
/// body. A tie within the similarity margin leaves `index` unpaired.
fn best_expected(
    index: usize,
    candidates: BTreeMap<usize, u32>,
    this: &Revision,
    other: &Revision,
    policy: Policy,
) -> Option<(usize, u32, u32)> {
    let body = &this.functions[index];
    let mut scored: Vec<(u32, u32, usize)> = candidates
        .into_iter()
        .filter(|(candidate, _)| comparable(body.size, other.functions[*candidate].size))
        .map(|(candidate, support)| {
            let ppm = similarity_ppm(&body.tokens, &other.functions[candidate].tokens);
            (support, ppm, candidate)
        })
        .filter(|(_, ppm, _)| *ppm >= policy.neighbourhood_minimum_ppm)
        .collect();
    scored.sort_by(|a, b| b.cmp(a));
    let (support, ppm, candidate) = *scored.first()?;
    if let Some((next_support, next_ppm, _)) = scored.get(1)
        && *next_support == support
        && ppm.saturating_sub(*next_ppm) < policy.margin_ppm
    {
        return None;
    }
    Some((candidate, support, ppm))
}
