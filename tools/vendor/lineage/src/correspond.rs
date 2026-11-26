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

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::Serialize;

use crate::archive::Revision;
use oer_tools_vendor_provenance::fingerprint::{Function, similarity_ppm};

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
        support: usize,
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
    let mut pairs: Vec<Pair> = state
        .pairs
        .into_iter()
        .map(|(left, found)| Pair {
            left: left.place,
            right: found.right.place,
            evidence: found.evidence,
            identical: found.identical,
            similarity_ppm: found.similarity_ppm,
        })
        .collect();
    pairs.sort_by_key(|pair| pair.right);
    pairs
}

/// A function of one revision with its place among the revision's
/// functions. The two come from the revision's own enumeration and stay
/// together; a node is told from the others of its revision, and ordered
/// among them, by its place.
#[derive(Clone, Copy)]
struct Node<'a> {
    place: usize,
    function: &'a Function,
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.place == other.place
    }
}

impl Eq for Node<'_> {}

impl PartialOrd for Node<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Node<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.place.cmp(&other.place)
    }
}

impl Hash for Node<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.place.hash(state);
    }
}

/// Every function of `revision` as a node.
fn nodes(revision: &Revision) -> impl Iterator<Item = Node<'_>> {
    revision
        .functions
        .iter()
        .enumerate()
        .map(|(place, function)| Node { place, function })
}

/// What a left function is paired with, and how.
struct Found<'a> {
    right: Node<'a>,
    evidence: Evidence,
    identical: bool,
    similarity_ppm: u32,
}

struct State<'a> {
    left: &'a Revision,
    right: &'a Revision,
    left_names: HashMap<&'a str, Node<'a>>,
    right_names: HashMap<&'a str, Node<'a>>,
    /// The pair of every paired left function.
    pairs: BTreeMap<Node<'a>, Found<'a>>,
    right_paired: HashSet<Node<'a>>,
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

    fn insert(&mut self, left: Node<'a>, right: Node<'a>, evidence: Evidence) {
        let identical = left.function.code == right.function.code;
        let similarity_ppm = match evidence {
            Evidence::Similar { ppm }
            | Evidence::Dominant { ppm, .. }
            | Evidence::Neighbourhood { ppm, .. } => ppm,
            _ if identical => 1_000_000,
            _ => similarity_ppm(&left.function.tokens, &right.function.tokens),
        };
        self.pairs.insert(
            left,
            Found {
                right,
                evidence,
                identical,
                similarity_ppm,
            },
        );
        self.right_paired.insert(right);
    }

    fn unpaired_left(&self) -> impl Iterator<Item = Node<'a>> + '_ {
        nodes(self.left).filter(|node| !self.pairs.contains_key(node))
    }

    fn unpaired_right(&self) -> impl Iterator<Item = Node<'a>> + '_ {
        nodes(self.right).filter(|node| !self.right_paired.contains(node))
    }

    fn pair_same_names(&mut self) {
        let matches: Vec<(Node<'a>, Node<'a>)> = self
            .left_names
            .iter()
            .filter_map(|(name, left)| self.right_names.get(name).map(|right| (*left, *right)))
            .collect();
        for (left, right) in matches {
            self.insert(left, right, Evidence::SameName);
        }
    }

    fn pair_exact_bodies(&mut self) {
        let mut left_bodies: HashMap<&str, Vec<Node<'a>>> = HashMap::new();
        for node in self.unpaired_left() {
            left_bodies
                .entry(node.function.code.as_str())
                .or_default()
                .push(node);
        }
        let mut right_bodies: HashMap<&str, Vec<Node<'a>>> = HashMap::new();
        for node in self.unpaired_right() {
            right_bodies
                .entry(node.function.code.as_str())
                .or_default()
                .push(node);
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
        let mut rejected: BTreeSet<(Node<'a>, Node<'a>)> = BTreeSet::new();
        loop {
            let mut votes: BTreeMap<Node<'a>, BTreeSet<Node<'a>>> = BTreeMap::new();
            let mut voters: BTreeMap<Node<'a>, BTreeSet<Node<'a>>> = BTreeMap::new();
            for (caller, found) in &self.pairs {
                for (left_callee, right_callee) in
                    self.corresponding_calls(&caller.function.calls, &found.right.function.calls)
                {
                    let (Some(&left), Some(&right)) = (
                        self.left_names.get(left_callee),
                        self.right_names.get(right_callee),
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
            // A vote stands where its left function has no other candidate
            // and its right function no other voter.
            let sole_candidates: BTreeSet<(Node<'a>, Node<'a>)> = votes
                .iter()
                .filter_map(|(left, rights)| Some((*left, single(rights)?)))
                .collect();
            let accepted: Vec<(Node<'a>, Node<'a>)> = voters
                .iter()
                .filter_map(|(right, lefts)| Some((single(lefts)?, *right)))
                .filter(|vote| sole_candidates.contains(vote))
                .collect();
            let mut progressed = false;
            for (left, right) in accepted {
                let score = similarity_ppm(&left.function.tokens, &right.function.tokens);
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
                .and_then(|node| self.pairs.get(node))
                .is_some_and(|found| self.right_names.get(right) == Some(&found.right))
    }

    /// The callees that correspond between two call lists: those at every
    /// position of equally long lists, otherwise those inside equally long
    /// gaps between the anchors of a longest common subsequence of
    /// corresponding callees.
    fn corresponding_calls<'c>(
        &self,
        left: &'c [String],
        right: &'c [String],
    ) -> Vec<(&'c str, &'c str)> {
        let callees = |(left, right): (&'c String, &'c String)| (left.as_str(), right.as_str());
        if left.len() == right.len() {
            return left.iter().zip(right).map(callees).collect();
        }
        let mut corresponding = Vec::new();
        let (mut lefts, mut rights) = (left.iter().enumerate(), right.iter().enumerate());
        // Each anchor ends a gap, and the lists' ends close the last one.
        let anchors = anchors(left, right, |left, right| self.anchored(left, right));
        let ends = anchors
            .iter()
            .map(|(left, right)| (Some(*left), Some(*right)))
            .chain([(None, None)]);
        for (left_end, right_end) in ends {
            // The callees up to the anchor, which is passed with them.
            let left_gap: Vec<&String> = lefts
                .by_ref()
                .take_while(|(place, _)| Some(*place) != left_end)
                .map(|(_, callee)| callee)
                .collect();
            let right_gap: Vec<&String> = rights
                .by_ref()
                .take_while(|(place, _)| Some(*place) != right_end)
                .map(|(_, callee)| callee)
                .collect();
            if left_gap.len() == right_gap.len() {
                corresponding.extend(left_gap.into_iter().zip(right_gap).map(callees));
            }
        }
        corresponding
    }

    fn pair_neighbourhood(&mut self, policy: Policy) {
        let left_calls = Calls::new(self.left, &self.left_names);
        let right_calls = Calls::new(self.right, &self.right_names);
        let left_to_right: HashMap<Node<'a>, Node<'a>> = self
            .pairs
            .iter()
            .map(|(left, found)| (*left, found.right))
            .collect();
        let right_to_left: HashMap<Node<'a>, Node<'a>> = self
            .pairs
            .iter()
            .map(|(left, found)| (found.right, *left))
            .collect();
        let left_paired: HashSet<Node<'a>> = self.pairs.keys().copied().collect();
        let accepted: Vec<(Node<'a>, Node<'a>, Evidence)> = self
            .unpaired_left()
            .filter_map(|left| {
                let candidates = expected(
                    left,
                    &left_calls,
                    &right_calls,
                    &left_to_right,
                    &self.right_paired,
                );
                let (right, support, ppm) = best_expected(left, candidates, policy)?;
                let candidates = expected(
                    right,
                    &right_calls,
                    &left_calls,
                    &right_to_left,
                    &left_paired,
                );
                let (back, _, _) = best_expected(right, candidates, policy)?;
                (back == left).then_some((left, right, Evidence::Neighbourhood { support, ppm }))
            })
            .collect();
        for (left, right, evidence) in accepted {
            self.insert(left, right, evidence);
        }
    }

    fn pair_similar(&mut self, policy: Policy) {
        let rights: Vec<(Node<'a>, HashSet<u64>)> = self
            .unpaired_right()
            .map(|node| (node, node.function.shingles()))
            .collect();
        // Best and second-best score per function, in both directions.
        let mut best_right: HashMap<Node<'a>, (Node<'a>, u32, u32)> = HashMap::new();
        let mut best_left: HashMap<Node<'a>, (Node<'a>, u32, u32)> = HashMap::new();
        for left in self.unpaired_left() {
            let shingles = left.function.shingles();
            let mut candidates: Vec<(u64, Node<'a>)> = rights
                .iter()
                .filter(|(right, _)| comparable(left.function.size, right.function.size))
                .map(|(right, other)| (jaccard_ppm(&shingles, other), *right))
                .collect();
            candidates.sort_by(|a, b| b.cmp(a));
            candidates.truncate(PREFILTER_CANDIDATES);
            for (_, right) in candidates {
                let score = similarity_ppm(&left.function.tokens, &right.function.tokens);
                record(&mut best_right, left, right, score);
                record(&mut best_left, right, left, score);
            }
        }
        let accepted: Vec<(Node<'a>, Node<'a>, Evidence)> = best_right
            .iter()
            .filter_map(|(left, (right, score, second))| {
                let (back, _, back_second) = best_left.get(right)?;
                if *back != *left {
                    return None;
                }
                let lead = |second: u32| score.saturating_sub(second) >= policy.margin_ppm;
                // The score is at least the ratio times the runner up's:
                // asked of the score's share, which a ratio of nothing does
                // not have, and which every score then meets.
                let dominates = |second: u32| {
                    score
                        .checked_div(policy.dominance_ratio)
                        .is_none_or(|share| share >= second)
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

/// One row of the table of the lengths that the ends of two lists have in
/// common: for the left items from one on, against the right items from
/// each column on. Past the last right item nothing is common, and a row
/// never falls towards the first column and rises by one at a time, so the
/// row is the columns where it rises. The row past the last left item has
/// none.
#[derive(Default)]
struct Row {
    /// The columns where the row rises, the last column first.
    rises: Vec<usize>,
}

impl Row {
    /// The common length with the right items from `column` on: the rises
    /// there or later.
    fn common_from(&self, column: usize) -> usize {
        self.rises.partition_point(|rise| *rise >= column)
    }

    /// The common length with the right items after `column`.
    fn common_after(&self, column: usize) -> usize {
        self.rises.partition_point(|rise| *rise > column)
    }

    /// The row of one more left item, which stands before this row's and
    /// corresponds to the right items at `columns`, the last column first.
    fn above(&self, columns: impl Iterator<Item = usize>) -> Self {
        let mut columns = columns.peekable();
        let mut rises = Vec::new();
        for rise in &self.rises {
            // The item and what was common after a column are one more than
            // was common from the last rise at or before it: of the columns
            // from this rise up to the one before it, the last takes the
            // rise's place. The others give the item a second place in the
            // same subsequence, and add nothing.
            let reached = columns.next_if(|column| column >= rise);
            rises.push(reached.unwrap_or(*rise));
            while columns.next_if(|column| column >= rise).is_some() {}
        }
        // A column before every rise: the row's greatest length grows.
        rises.extend(columns.next());
        rises.shrink_to_fit();
        Self { rises }
    }
}

/// The rows of two lists, the first left item's first, and after the last
/// item's the row that has nothing in common.
fn rows<L, R>(left: &[L], right: &[R], corresponds: impl Fn(&L, &R) -> bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut below = Row::default();
    for item in left.iter().rev() {
        let columns = right
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, other)| corresponds(item, other))
            .map(|(column, _)| column);
        let above = below.above(columns);
        rows.push(std::mem::replace(&mut below, above));
    }
    rows.push(below);
    rows.reverse();
    rows
}

/// The places of the items that anchor the alignment of two lists, in
/// order: a longest common subsequence of corresponding items.
///
/// From the lists' starts: two items that correspond are an anchor, and
/// both are passed. Otherwise the left item is passed where the lists have
/// as much in common without it as without the right one, and the right
/// item where they have more without that one.
fn anchors<L, R>(
    left: &[L],
    right: &[R],
    corresponds: impl Fn(&L, &R) -> bool,
) -> Vec<(usize, usize)> {
    let rows = rows(left, right, &corresponds);
    // Every left item with its row and the row below it.
    let mut lefts = left.iter().enumerate().zip(rows.array_windows()).peekable();
    let mut rights = right.iter().enumerate().peekable();
    let mut anchors = Vec::new();
    while let (Some(&((place, item), [row, below])), Some(&(column, other))) =
        (lefts.peek(), rights.peek())
    {
        // A common length passes the one after both items by one at most,
        // so "it is that and one" is "it is greater".
        if corresponds(item, other) && row.common_from(column) > below.common_after(column) {
            anchors.push((place, column));
            lefts.next();
            rights.next();
        } else if below.common_from(column) >= row.common_after(column) {
            lefts.next();
        } else {
            rights.next();
        }
    }
    anchors
}

fn unique_names(revision: &Revision) -> HashMap<&str, Node<'_>> {
    let mut seen: HashMap<&str, Option<Node<'_>>> = HashMap::new();
    for node in nodes(revision) {
        seen.entry(node.function.name.as_str())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(node));
    }
    seen.into_iter()
        .filter_map(|(name, node)| node.map(|node| (name, node)))
        .collect()
}

fn single<'a>(set: &BTreeSet<Node<'a>>) -> Option<Node<'a>> {
    let mut iter = set.iter();
    match (iter.next(), iter.next()) {
        (Some(value), None) => Some(*value),
        _ => None,
    }
}

/// Whether two bodies of these sizes are compared at all: the larger is at
/// most twice the smaller and sixteen bytes. A size whose double no `usize`
/// holds is compared with nothing.
fn comparable(left: usize, right: usize) -> bool {
    let (small, large) = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    small
        .checked_mul(2)
        .and_then(|twice| twice.checked_add(16))
        .is_some_and(|limit| large <= limit)
}

fn jaccard_ppm(left: &HashSet<u64>, right: &HashSet<u64>) -> u64 {
    // In a width that holds the product. Two empty sets have no quotient,
    // and share nothing.
    let wide = |count: usize| u128::try_from(count).ok();
    wide(left.intersection(right).count())
        .and_then(|shared| shared.checked_mul(1_000_000))
        .zip(wide(left.union(right).count()))
        .and_then(|(shared, union)| shared.checked_div(union))
        .and_then(|ppm| u64::try_from(ppm).ok())
        .unwrap_or(0)
}

/// Keep the best candidate and the best score among the others.
fn record<'a>(
    table: &mut HashMap<Node<'a>, (Node<'a>, u32, u32)>,
    key: Node<'a>,
    candidate: Node<'a>,
    score: u32,
) {
    let entry = table.entry(key).or_insert((candidate, score, 0));
    if entry.0 == candidate {
        entry.1 = entry.1.max(score);
    } else if score > entry.1 {
        *entry = (candidate, score, entry.1);
    } else {
        entry.2 = entry.2.max(score);
    }
}

/// Call relations between the uniquely named functions of one revision. A
/// function that calls none of them, or that none of them calls, has no
/// entry.
struct Calls<'a> {
    callees: HashMap<Node<'a>, BTreeSet<Node<'a>>>,
    callers: HashMap<Node<'a>, BTreeSet<Node<'a>>>,
}

impl<'a> Calls<'a> {
    fn new(revision: &'a Revision, names: &HashMap<&str, Node<'a>>) -> Self {
        let mut callees: HashMap<Node<'a>, BTreeSet<Node<'a>>> = HashMap::new();
        let mut callers: HashMap<Node<'a>, BTreeSet<Node<'a>>> = HashMap::new();
        for caller in nodes(revision) {
            for callee in &caller.function.calls {
                if let Some(&callee) = names.get(callee.as_str()) {
                    callees.entry(caller).or_default().insert(callee);
                    callers.entry(callee).or_default().insert(caller);
                }
            }
        }
        Self { callees, callers }
    }
}

/// Unpaired functions of the other revision where `node` is expected, with
/// the number of relations that support each: the callees of each paired
/// caller's counterpart and the callers of each paired callee's counterpart.
fn expected<'a>(
    node: Node<'a>,
    this: &Calls<'a>,
    other: &Calls<'a>,
    counterpart: &HashMap<Node<'a>, Node<'a>>,
    other_paired: &HashSet<Node<'a>>,
) -> Vec<(Node<'a>, usize)> {
    // Every candidate with the counterparts that expect it, one for each
    // relation it is expected through.
    let mut expecting: BTreeMap<Node<'a>, Vec<Node<'a>>> = BTreeMap::new();
    let related = [
        (&this.callers, &other.callees),
        (&this.callees, &other.callers),
    ];
    for (neighbours, relation) in related {
        let paired = neighbours
            .get(&node)
            .into_iter()
            .flatten()
            .filter_map(|neighbour| counterpart.get(neighbour));
        for paired in paired {
            let candidates = relation
                .get(paired)
                .into_iter()
                .flatten()
                .filter(|candidate| !other_paired.contains(candidate));
            for candidate in candidates {
                expecting.entry(*candidate).or_default().push(*paired);
            }
        }
    }
    expecting
        .into_iter()
        .map(|(candidate, by)| (candidate, by.len()))
        .collect()
}

/// The unique best expected candidate above the similarity floor, as
/// (candidate, support, similarity): the most support, then the most similar
/// body. A tie within the similarity margin leaves `node` unpaired.
fn best_expected<'a>(
    node: Node<'_>,
    candidates: Vec<(Node<'a>, usize)>,
    policy: Policy,
) -> Option<(Node<'a>, usize, u32)> {
    let body = node.function;
    let mut scored: Vec<(usize, u32, Node<'a>)> = candidates
        .into_iter()
        .filter(|(candidate, _)| comparable(body.size, candidate.function.size))
        .map(|(candidate, support)| {
            let ppm = similarity_ppm(&body.tokens, &candidate.function.tokens);
            (support, ppm, candidate)
        })
        .filter(|(_, ppm, _)| *ppm >= policy.neighbourhood_minimum_ppm)
        .collect();
    scored.sort_by(|a, b| b.cmp(a));
    let mut ranked = scored.into_iter();
    let (support, ppm, candidate) = ranked.next()?;
    if ranked.next().is_some_and(|(next_support, next_ppm, _)| {
        next_support == support && ppm.saturating_sub(next_ppm) < policy.margin_ppm
    }) {
        return None;
    }
    Some((candidate, support, ppm))
}

#[cfg(test)]
mod tests {
    use super::{anchors, comparable, jaccard_ppm, rows};
    use std::collections::HashSet;

    /// Every list of up to `length` items out of three.
    fn lists(length: usize) -> Vec<Vec<u8>> {
        let mut all = vec![vec![]];
        let mut last = vec![vec![]];
        for _ in 0..length {
            last = last
                .iter()
                .flat_map(|list: &Vec<u8>| {
                    (0..3).map(move |item| {
                        let mut longer = list.clone();
                        longer.push(item);
                        longer
                    })
                })
                .collect();
            all.extend(last.iter().cloned());
        }
        all
    }

    /// Items that correspond as names do, and as a pair of the
    /// correspondence does beside them: the left 0 also with the right 1.
    const RELATIONS: [fn(&u8, &u8) -> bool; 2] = [
        |left, right| left == right,
        |left, right| left == right || (*left == 0 && *right == 1),
    ];

    /// The table of common lengths cell by cell: `table[i][j]` for the left
    /// items from `i` on and the right items from `j` on.
    fn table(left: &[u8], right: &[u8], corresponds: fn(&u8, &u8) -> bool) -> Vec<Vec<usize>> {
        let mut lengths = vec![vec![0; right.len() + 1]; left.len() + 1];
        for (i, item) in left.iter().enumerate().rev() {
            for (j, other) in right.iter().enumerate().rev() {
                lengths[i][j] = if corresponds(item, other) {
                    lengths[i + 1][j + 1] + 1
                } else {
                    lengths[i + 1][j].max(lengths[i][j + 1])
                };
            }
        }
        lengths
    }

    #[test]
    fn a_row_s_rises_count_the_table_s_common_lengths_for_every_pair_of_short_lists() {
        let lists = lists(4);
        assert_eq!(lists.len(), 121);
        for corresponds in RELATIONS {
            for left in &lists {
                for right in &lists {
                    let table = table(left, right, corresponds);
                    let rows = rows(left, right, corresponds);
                    assert_eq!(rows.len(), table.len(), "{left:?} {right:?}");
                    for (i, row) in rows.iter().enumerate() {
                        for j in 0..=right.len() {
                            assert_eq!(
                                row.common_from(j),
                                table[i][j],
                                "{left:?} {right:?} {i} {j}"
                            );
                            let after = table[i].get(j + 1).copied().unwrap_or(0);
                            assert_eq!(row.common_after(j), after, "{left:?} {right:?} {i} {j}");
                        }
                        // The rises stand in their order, each once.
                        assert!(
                            row.rises
                                .array_windows()
                                .all(|[later, sooner]| later > sooner),
                            "{left:?} {right:?} {:?}",
                            row.rises
                        );
                    }
                    // A cell passes the one after both items by one at
                    // most, and is that and one exactly where it is greater.
                    for i in 0..left.len() {
                        for j in 0..right.len() {
                            assert!(table[i][j] <= table[i + 1][j + 1] + 1);
                            assert_eq!(
                                rows[i].common_from(j) > rows[i + 1].common_after(j),
                                table[i][j] == table[i + 1][j + 1] + 1,
                                "{left:?} {right:?} {i} {j}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_anchors_are_those_of_the_walk_through_the_table_for_every_pair_of_short_lists() {
        for corresponds in RELATIONS {
            for left in &lists(4) {
                for right in &lists(4) {
                    let lengths = table(left, right, corresponds);
                    let mut walked = Vec::new();
                    let (mut i, mut j) = (0, 0);
                    while i < left.len() && j < right.len() {
                        if corresponds(&left[i], &right[j])
                            && lengths[i][j] == lengths[i + 1][j + 1] + 1
                        {
                            walked.push((i, j));
                            i += 1;
                            j += 1;
                        } else if lengths[i + 1][j] >= lengths[i][j + 1] {
                            i += 1;
                        } else {
                            j += 1;
                        }
                    }
                    assert_eq!(
                        anchors(left, right, corresponds),
                        walked,
                        "{left:?} {right:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_shared_part_of_two_sets_is_held_in_parts_per_million() {
        let set = |tokens: &[u64]| tokens.iter().copied().collect::<HashSet<u64>>();
        for (left, right, expected) in [
            (&[1, 2, 3][..], &[2, 3, 4][..], 500_000),
            (&[1, 2, 3], &[1, 2, 3], 1_000_000),
            (&[1, 2, 3], &[3], 333_333),
            (&[1, 2], &[3, 4], 0),
            (&[1], &[], 0),
            // Nothing to compare.
            (&[], &[], 0),
        ] {
            assert_eq!(
                jaccard_ppm(&set(left), &set(right)),
                expected,
                "{left:?} {right:?}"
            );
            assert_eq!(
                jaccard_ppm(&set(right), &set(left)),
                expected,
                "{right:?} {left:?}"
            );
        }
    }

    #[test]
    fn sizes_are_comparable_up_to_twice_the_smaller_and_sixteen_bytes() {
        for (left, right, expected) in [
            (10, 36, true),
            (10, 37, false),
            (0, 16, true),
            (0, 17, false),
            (5, 5, true),
            (0, 0, true),
        ] {
            assert_eq!(comparable(left, right), expected, "{left} {right}");
            assert_eq!(comparable(right, left), expected, "{right} {left}");
        }
    }

    #[test]
    fn a_size_whose_double_no_usize_holds_is_compared_with_nothing() {
        let half = usize::MAX / 2;
        // Twice this and sixteen is the last sum a `usize` holds.
        assert!(comparable(half - 8, usize::MAX - 1));
        assert!(!comparable(half - 7, usize::MAX));
        assert!(!comparable(half, usize::MAX));
        assert!(!comparable(usize::MAX, usize::MAX));
    }
}
