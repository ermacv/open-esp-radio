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
use std::rc::Rc;

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
        let anchors = self.anchors(left, right);
        let ends = anchors
            .iter()
            .rev()
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

    /// The places of the callees that anchor the alignment of two call
    /// lists, the last anchor first.
    ///
    /// The alignment of two lists starts with their first callees where
    /// these correspond, and goes on with what follows both. Where they do
    /// not, it is the longer of the alignment without the left list's first
    /// callee and the one without the right list's, the former where they
    /// are equally long. A list that has ended aligns nothing.
    ///
    /// A row holds, for the left callees from one on, the alignment with
    /// what follows each right callee; it is made from the row of the left
    /// callees after that one, and a cell holds its alignment whole, so the
    /// length that decides between two is the length of what they hold.
    fn anchors(&self, left: &[String], right: &[String]) -> Rc<Vec<(usize, usize)>> {
        let nothing: Rc<Vec<(usize, usize)>> = Rc::default();
        let mut below: Vec<Rc<Vec<(usize, usize)>>> =
            right.iter().map(|_| Rc::clone(&nothing)).collect();
        for (left_place, left_callee) in left.iter().enumerate().rev() {
            // Past the last right callee nothing is aligned: in the row
            // below, and in this one.
            let mut diagonal = Rc::clone(&nothing);
            let mut beside = Rc::clone(&nothing);
            let mut row = Vec::with_capacity(right.len());
            for ((right_place, right_callee), under) in right.iter().enumerate().zip(&below).rev() {
                let cell = if self.anchored(left_callee, right_callee) {
                    let mut longer = Vec::clone(&diagonal);
                    longer.push((left_place, right_place));
                    Rc::new(longer)
                } else if under.len() >= beside.len() {
                    Rc::clone(under)
                } else {
                    Rc::clone(&beside)
                };
                diagonal = Rc::clone(under);
                beside = Rc::clone(&cell);
                row.push(cell);
            }
            row.reverse();
            below = row;
        }
        // A right list without a callee has no cell, and anchors nothing.
        below.first().map_or(nothing, Rc::clone)
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
    use super::{comparable, jaccard_ppm};
    use std::collections::HashSet;

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
