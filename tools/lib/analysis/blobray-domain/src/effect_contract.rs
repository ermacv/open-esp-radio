//! Finite reviewed classification of concrete MMIO/delay/fence observations.
use crate::*;
pub const MAX_EFFECT_RULES: usize = 128;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Interpretation ceiling of MATCH; neither variant establishes general equivalence.
pub enum EffectClaimCeiling {
    SelectedEffectEquality,
    ReviewedEffectRefinement,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EffectSelector {
    MmioRead { address: u32, width: u8 },
    MmioWrite { address: u32, width: u8 },
    Delay { micros: Option<u32> },
    Fence { predecessor: u8, successor: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// Exact constrains each side; Any leaves values unconstrained by the rule.
/// Required and retained Omitted events still compare their values exactly.
pub enum EffectValue {
    Any,
    Exact { value: u32 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPattern {
    pub selector: EffectSelector,
    pub value: EffectValue,
    /// The pattern applies only when the side's next concrete effect matches
    /// this selector, for example a plumbing delay before a transport read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub followed_by: Option<EffectSelector>,
    /// The pattern applies only when the side's previous concrete effect
    /// matches this selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preceded_by: Option<EffectSelector>,
    /// The pattern applies only to this occurrence, counting from one, of
    /// an effect `selector` matches on its side: two samples of one register
    /// with different roles get different rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<u32>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectDisposition {
    Required,
    /// Replacement may retain the exact effect in order, or omit it.
    Omitted,
    /// Both sides must satisfy their explicit patterns; values are not implicitly equated.
    Replaced,
    Added,
    Forbidden,
    /// Implementation plumbing on either side: matching effects remain raw
    /// evidence but are neither paired nor value-constrained.
    Ignored,
}
/// Treatment of concrete effects that no rule selects.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnclassifiedEffects {
    /// Every effect must be classified; an unclassified one is INCOMPLETE.
    #[default]
    Incomplete,
    /// Effects outside the rules compare exactly, in order and value.
    Required,
}
impl UnclassifiedEffects {
    fn is_incomplete(&self) -> bool {
        *self == Self::Incomplete
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRule {
    pub name: String,
    pub vendor: Option<EffectPattern>,
    pub replacement: Option<EffectPattern>,
    pub disposition: EffectDisposition,
    /// Exercise obligation per side. Zero means required only when observed.
    /// The replacement side of an omitted rule always has minimum zero.
    pub min_occurrences: u32,
    /// Observed excess is a known difference, including in an unfinished run.
    pub max_occurrences: u32,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectContract {
    pub vendor: CallEndpoint,
    pub replacement: CallEndpoint,
    pub rules: Vec<EffectRule>,
    #[serde(default, skip_serializing_if = "UnclassifiedEffects::is_incomplete")]
    pub unclassified: UnclassifiedEffects,
    pub claim_ceiling: EffectClaimCeiling,
    pub applicability: String,
    pub reason: String,
}
/// A contract supplied with a comparison, with the digest of its canonical
/// encoding a relation selects it by.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedEffectContract {
    pub id: ArtifactId,
    pub contract: EffectContract,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EffectGap {
    Unclassified { replacement: bool, event: u32 },
    Unexercised { replacement: bool, rule: u16 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectViolationKind {
    Forbidden,
    Value,
    Count,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectViolation {
    pub replacement: bool,
    pub event: u32,
    pub rule: u16,
    pub kind: EffectViolationKind,
}
fn invalid() -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        "invalid reviewed effect contract",
    )
}
impl EffectSelector {
    fn matches(self, event: &ExecutionEvent) -> bool {
        match (self, event) {
            (
                Self::MmioRead {
                    address: a,
                    width: w,
                },
                ExecutionEvent::Read { address, width, .. },
            )
            | (
                Self::MmioWrite {
                    address: a,
                    width: w,
                },
                ExecutionEvent::Write { address, width, .. },
            ) => a == *address && w == *width,
            (Self::Delay { micros }, ExecutionEvent::DelayMicros { value }) => {
                micros.is_none_or(|n| n == *value)
            }
            (
                Self::Fence {
                    predecessor: p,
                    successor: s,
                },
                ExecutionEvent::Fence {
                    predecessor,
                    successor,
                },
            ) => p == *predecessor && s == *successor,
            _ => false,
        }
    }
    fn overlaps(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::MmioRead {
                    address: a,
                    width: aw,
                },
                Self::MmioRead {
                    address: b,
                    width: bw,
                },
            )
            | (
                Self::MmioWrite {
                    address: a,
                    width: aw,
                },
                Self::MmioWrite {
                    address: b,
                    width: bw,
                },
            ) => {
                u64::from(a) < u64::from(b) + u64::from(bw)
                    && u64::from(b) < u64::from(a) + u64::from(aw)
            }
            (Self::Delay { micros: a }, Self::Delay { micros: b }) => {
                a.is_none() || b.is_none() || a == b
            }
            _ => self == other,
        }
    }
}
impl EffectPattern {
    fn validate(self) -> Result<()> {
        for neighbour in [self.followed_by, self.preceded_by].into_iter().flatten() {
            EffectPattern {
                selector: neighbour,
                value: EffectValue::Any,
                followed_by: None,
                preceded_by: None,
                occurrence: None,
            }
            .validate()?;
        }
        if self.occurrence == Some(0) {
            return Err(invalid());
        }
        match self.selector {
            EffectSelector::MmioRead { address, width }
            | EffectSelector::MmioWrite { address, width } => {
                if !matches!(width, 1 | 2 | 4)
                    || !address.is_multiple_of(u32::from(width))
                    || u64::from(address) + u64::from(width) > 1u64 << 32
                    || (width < 4
                        && matches!(self.value,EffectValue::Exact {value} if value >> (width*8)!=0))
                {
                    return Err(invalid());
                }
            }
            EffectSelector::Fence {
                predecessor,
                successor,
            } => {
                if predecessor > 15 || successor > 15 || self.value != EffectValue::Any {
                    return Err(invalid());
                }
            }
            EffectSelector::Delay { micros: Some(n) } => {
                if matches!(self.value,EffectValue::Exact {value} if value!=n) {
                    return Err(invalid());
                }
            }
            EffectSelector::Delay { micros: None } => (),
        }
        Ok(())
    }
    /// The pattern selects `event` whose immediately next concrete effect is
    /// `next`, without a predecessor or occurrence constraint.
    pub fn selects(self, event: &ExecutionEvent, next: Option<&ExecutionEvent>) -> bool {
        self.selects_after(event, next, &[])
    }
    /// The pattern selects `event`, which follows the side's earlier concrete
    /// effects `history` and precedes `next`.
    pub fn selects_after(
        self,
        event: &ExecutionEvent,
        next: Option<&ExecutionEvent>,
        history: &[ExecutionEvent],
    ) -> bool {
        let earlier = history.iter().filter(|e| self.selector.matches(e)).count();
        self.selects_following(event, next, history.last(), earlier as u64)
    }
    /// The pattern selects `event`, which directly follows `previous` after
    /// `earlier` effects its selector matches, and precedes `next`.
    pub fn selects_following(
        self,
        event: &ExecutionEvent,
        next: Option<&ExecutionEvent>,
        previous: Option<&ExecutionEvent>,
        earlier: u64,
    ) -> bool {
        self.selector.matches(event)
            && self
                .followed_by
                .is_none_or(|f| next.is_some_and(|n| f.matches(n)))
            && self
                .preceded_by
                .is_none_or(|p| previous.is_some_and(|previous| p.matches(previous)))
            && self.occurrence.is_none_or(|n| 1 + earlier == u64::from(n))
    }
    /// Two patterns can select the same effect.
    pub fn overlaps(self, other: Self) -> bool {
        let neighbours = |a: Option<EffectSelector>, b: Option<EffectSelector>| match (a, b) {
            (Some(a), Some(b)) => a.overlaps(b),
            _ => true,
        };
        self.selector.overlaps(other.selector)
            && neighbours(self.followed_by, other.followed_by)
            && neighbours(self.preceded_by, other.preceded_by)
            && match (self.occurrence, other.occurrence) {
                // Distinct occurrences of one selector never coincide.
                (Some(a), Some(b)) => a == b || self.selector != other.selector,
                _ => true,
            }
    }
    fn accepts(self, event: &ExecutionEvent) -> bool {
        match self.value {
            EffectValue::Any => true,
            EffectValue::Exact { value: expected } => {
                matches!(event,ExecutionEvent::Read {value,..}|ExecutionEvent::Write {value,..}|ExecutionEvent::DelayMicros {value} if *value==expected)
            }
        }
    }
}
impl EffectRule {
    fn pattern(&self, replacement: bool) -> Option<EffectPattern> {
        if replacement {
            self.replacement
        } else {
            self.vendor
        }
    }
    fn minimum(&self, replacement: bool) -> u32 {
        if self.pattern(replacement).is_none()
            || (replacement && self.disposition == EffectDisposition::Omitted)
        {
            0
        } else {
            self.min_occurrences
        }
    }
}
impl EffectContract {
    pub fn validate(&self) -> Result<()> {
        self.vendor.validate()?;
        self.replacement.validate()?;
        if !matches!(self.vendor.boundary, ReviewedCallBoundary::Code { .. })
            || !matches!(self.replacement.boundary, ReviewedCallBoundary::Code { .. })
            || self.rules.is_empty()
            || self.rules.len() > MAX_EFFECT_RULES
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
            || self.reason.trim().is_empty()
            || self.reason.len() > 4096
        {
            return Err(invalid());
        }
        for (i, r) in self.rules.iter().enumerate() {
            if r.name.trim().is_empty()
                || r.name.len() > 128
                || !r.name.is_ascii()
                || r.reason.trim().is_empty()
                || r.reason.len() > 4096
                || r.min_occurrences > r.max_occurrences
                || self.rules[..i].iter().any(|p| p.name == r.name)
            {
                return Err(invalid());
            }
            let shape = match r.disposition {
                EffectDisposition::Required | EffectDisposition::Omitted => {
                    r.vendor.is_some() && r.vendor == r.replacement && r.max_occurrences > 0
                }
                EffectDisposition::Ignored => {
                    r.vendor.is_some()
                        && r.vendor == r.replacement
                        && r.max_occurrences > 0
                        && r.vendor.is_some_and(|p| p.value == EffectValue::Any)
                }
                EffectDisposition::Replaced => {
                    r.vendor.is_some() && r.replacement.is_some() && r.max_occurrences > 0
                }
                EffectDisposition::Added => {
                    r.vendor.is_none() && r.replacement.is_some() && r.max_occurrences > 0
                }
                EffectDisposition::Forbidden => {
                    r.vendor.is_some()
                        && r.vendor == r.replacement
                        && r.min_occurrences == 0
                        && r.max_occurrences == 0
                        && r.vendor.is_some_and(|p| p.value == EffectValue::Any)
                }
            };
            if !shape
                || (self.claim_ceiling == EffectClaimCeiling::SelectedEffectEquality
                    && !matches!(
                        r.disposition,
                        EffectDisposition::Required | EffectDisposition::Forbidden
                    ))
            {
                return Err(invalid());
            }
            for side in [false, true] {
                if let Some(p) = r.pattern(side) {
                    p.validate()?;
                    if self.rules[..i]
                        .iter()
                        .filter_map(|r| r.pattern(side))
                        .any(|q| p.overlaps(q))
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.vendor.allocated_bytes()
            + self.replacement.allocated_bytes()
            + self.applicability.capacity() as u64
            + self.reason.capacity() as u64
            + (self.rules.capacity() * std::mem::size_of::<EffectRule>()) as u64
            + self
                .rules
                .iter()
                .map(|r| (r.name.capacity() + r.reason.capacity()) as u64)
                .sum::<u64>()
    }
    pub fn validate_use(&self, request: &ExecutionRequest, phase: usize) -> Result<()> {
        self.validate()?;
        let case = request.cases.get(phase).ok_or_else(invalid)?;
        let relation = case.relation.as_ref().ok_or_else(invalid)?;
        let right = request.replacement.as_ref().ok_or_else(invalid)?;
        let input = case.replacement.as_ref().ok_or_else(invalid)?;
        if !self.vendor.in_target(&request.vendor)
            || !self.replacement.in_target(right)
            || self.vendor.address() != case.vendor.entry
            || self.replacement.address() != input.entry
            || !(relation.events.mmio_read
                && relation.events.mmio_write
                && relation.events.fence
                && relation.events.delay)
            || self
                .rules
                .iter()
                .any(|r| r.max_occurrences > request.max_events)
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSelection {
    Required(u16),
    /// Selected by no rule under `unclassified: required`: compared exactly.
    Unlisted,
    Ignored(u16),
    Omitted(u16),
    Replaced(u16),
    Added(u16),
    Unclassified,
    Violation(EffectViolation),
}
/// Bounded per-side policy state shared by verification and retained admission.
/// The selector identity of a concrete effect: every selector that matches
/// the effect has this key, except a delay selector without a value.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EffectKey {
    Read(u32, u8),
    Write(u32, u8),
    Delay(u32),
    Fence(u8, u8),
}
impl EffectKey {
    fn of_event(event: &ExecutionEvent) -> Option<Self> {
        Some(match *event {
            ExecutionEvent::Read { address, width, .. } => Self::Read(address, width),
            ExecutionEvent::Write { address, width, .. } => Self::Write(address, width),
            ExecutionEvent::DelayMicros { value } => Self::Delay(value),
            ExecutionEvent::Fence {
                predecessor,
                successor,
            } => Self::Fence(predecessor, successor),
            _ => return None,
        })
    }
    /// The key of the effects `selector` matches; `None` for any delay.
    fn of_selector(selector: EffectSelector) -> Option<Self> {
        Some(match selector {
            EffectSelector::MmioRead { address, width } => Self::Read(address, width),
            EffectSelector::MmioWrite { address, width } => Self::Write(address, width),
            EffectSelector::Delay { micros } => Self::Delay(micros?),
            EffectSelector::Fence {
                predecessor,
                successor,
            } => Self::Fence(predecessor, successor),
        })
    }
}
/// The side's rules by the effects their selectors match, each list in rule
/// order, so an effect is tested only against rules that can select it.
#[derive(Default)]
struct RuleIndex {
    keyed: std::collections::BTreeMap<EffectKey, Vec<usize>>,
    any_delay: Vec<usize>,
}
impl RuleIndex {
    fn new(rules: &[EffectRule], replacement: bool) -> Self {
        let mut index = Self::default();
        for (i, rule) in rules.iter().enumerate() {
            let Some(pattern) = rule.pattern(replacement) else {
                continue;
            };
            match EffectKey::of_selector(pattern.selector) {
                Some(key) => index.keyed.entry(key).or_default().push(i),
                None => index.any_delay.push(i),
            }
        }
        index
    }
    /// Indices of the rules whose selector matches `event`, ascending.
    fn candidates(&self, event: &ExecutionEvent) -> impl Iterator<Item = usize> + '_ {
        let keyed = EffectKey::of_event(event)
            .and_then(|key| self.keyed.get(&key))
            .map_or(&[][..], Vec::as_slice);
        let any = if matches!(event, ExecutionEvent::DelayMicros { .. }) {
            &self.any_delay[..]
        } else {
            &[]
        };
        let (mut a, mut b) = (keyed.iter().peekable(), any.iter().peekable());
        std::iter::from_fn(move || match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if x < y => a.next().copied(),
            (Some(_), Some(_)) | (None, Some(_)) => b.next().copied(),
            (Some(_), None) => a.next().copied(),
            (None, None) => None,
        })
    }
}
pub struct EffectTracker<'a> {
    contract: &'a EffectContract,
    rules: RuleIndex,
    replacement: bool,
    counts: [u32; MAX_EFFECT_RULES],
    first_unclassified: Option<EffectGap>,
    first_violation: Option<EffectViolation>,
    /// The side's last concrete effect, for predecessor patterns.
    previous: Option<ExecutionEvent>,
    /// Earlier effects of the side each rule's selector matches, for
    /// occurrence patterns.
    earlier: [u64; MAX_EFFECT_RULES],
}
impl<'a> EffectTracker<'a> {
    pub fn new(
        contract: &'a EffectContract,
        replacement: bool,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        c.checkpoint((contract.rules.len() as u64 + 1).saturating_pow(2))?;
        contract.validate()?;
        Ok(Self {
            contract,
            rules: RuleIndex::new(&contract.rules, replacement),
            replacement,
            counts: [0; MAX_EFFECT_RULES],
            first_unclassified: None,
            first_violation: None,
            previous: None,
            earlier: [0; MAX_EFFECT_RULES],
        })
    }
    /// Classify `event`; `next` is the same side's next concrete effect, if any.
    pub fn observe(
        &mut self,
        event: &ExecutionEvent,
        next: Option<&ExecutionEvent>,
        ordinal: u32,
        c: &mut dyn RunControl,
    ) -> Result<EffectSelection> {
        c.checkpoint(self.contract.rules.len() as u64 + 1)?;
        let canonical = match event {
            ExecutionEvent::Read {
                address,
                width,
                value,
            }
            | ExecutionEvent::Write {
                address,
                width,
                value,
            } => {
                matches!(width, 1 | 2 | 4)
                    && address.is_multiple_of(u32::from(*width))
                    && (*width == 4 || *value >> (*width * 8) == 0)
            }
            ExecutionEvent::Fence {
                predecessor,
                successor,
            } => *predecessor <= 15 && *successor <= 15,
            ExecutionEvent::DelayMicros { .. } => true,
            _ => false,
        };
        if !canonical {
            return Err(Error::new(
                ErrorCode::Integrity,
                "invalid concrete effect observation",
            ));
        }
        let (previous, earlier) = (self.previous.as_ref(), &self.earlier);
        let selected = self.rules.candidates(event).find_map(|i| {
            let r = &self.contract.rules[i];
            r.pattern(self.replacement)
                .filter(|p| {
                    let earlier = earlier.get(i).copied().unwrap_or_default();
                    p.selects_following(event, next, previous, earlier)
                })
                .map(|p| (i, r, p))
        });
        for i in self.rules.candidates(event) {
            if let Some(count) = self.earlier.get_mut(i) {
                *count += 1;
            }
        }
        self.previous = Some(event.clone());
        let Some((i, r, p)) = selected else {
            if self.contract.unclassified == UnclassifiedEffects::Required {
                return Ok(EffectSelection::Unlisted);
            }
            self.first_unclassified
                .get_or_insert(EffectGap::Unclassified {
                    replacement: self.replacement,
                    event: ordinal,
                });
            return Ok(EffectSelection::Unclassified);
        };
        if i >= MAX_EFFECT_RULES {
            return Err(invalid());
        }
        self.counts[i] = self.counts[i].checked_add(1).ok_or_else(invalid)?;
        let violation = if r.disposition == EffectDisposition::Forbidden {
            Some(EffectViolationKind::Forbidden)
        } else if !p.accepts(event) {
            Some(EffectViolationKind::Value)
        } else if self.counts[i] > r.max_occurrences {
            Some(EffectViolationKind::Count)
        } else {
            None
        };
        if let Some(kind) = violation {
            let violation = EffectViolation {
                replacement: self.replacement,
                event: ordinal,
                rule: i as u16,
                kind,
            };
            self.first_violation.get_or_insert(violation);
            return Ok(EffectSelection::Violation(violation));
        }
        Ok(match r.disposition {
            EffectDisposition::Required => EffectSelection::Required(i as u16),
            EffectDisposition::Omitted => EffectSelection::Omitted(i as u16),
            EffectDisposition::Replaced => EffectSelection::Replaced(i as u16),
            EffectDisposition::Added => EffectSelection::Added(i as u16),
            EffectDisposition::Ignored => EffectSelection::Ignored(i as u16),
            EffectDisposition::Forbidden => unreachable!(),
        })
    }
    pub fn claim_ceiling(&self) -> EffectClaimCeiling {
        self.contract.claim_ceiling
    }
    pub fn first_violation(&self) -> Option<EffectViolation> {
        self.first_violation
    }
    pub fn gap(&self) -> Option<EffectGap> {
        self.first_unclassified.or_else(|| {
            self.contract
                .rules
                .iter()
                .enumerate()
                .find(|(i, r)| {
                    *i >= MAX_EFFECT_RULES || self.counts[*i] < r.minimum(self.replacement)
                })
                .map(|(i, _)| EffectGap::Unexercised {
                    replacement: self.replacement,
                    rule: i as u16,
                })
        })
    }
}
pub fn is_contract_effect(event: &ExecutionEvent) -> bool {
    matches!(
        event,
        ExecutionEvent::Read { .. }
            | ExecutionEvent::Write { .. }
            | ExecutionEvent::DelayMicros { .. }
            | ExecutionEvent::Fence { .. }
    )
}
pub fn selected_effect_contract<'a>(
    relation: Option<&ComparisonRelation>,
    contracts: &'a [ResolvedEffectContract],
) -> Result<Option<&'a ResolvedEffectContract>> {
    let Some(id) = relation.and_then(|r| r.effects.as_ref()) else {
        return Ok(None);
    };
    let mut matches = contracts.iter().filter(|p| &p.id == id);
    let selected = matches.next().ok_or_else(|| {
        Error::new(
            ErrorCode::Integrity,
            "selected resolved effect contract missing",
        )
    })?;
    if matches.next().is_some() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "duplicate resolved effect contract",
        ));
    }
    Ok(Some(selected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_candidates_are_the_matching_rules_in_rule_order() {
        let read = |address| EffectSelector::MmioRead { address, width: 4 };
        let selectors = [
            EffectSelector::Delay { micros: None },
            read(0x10),
            EffectSelector::Delay { micros: Some(5) },
            EffectSelector::MmioWrite {
                address: 0x10,
                width: 4,
            },
            read(0x10),
            EffectSelector::Delay { micros: None },
            EffectSelector::Fence {
                predecessor: 3,
                successor: 3,
            },
            EffectSelector::Delay { micros: Some(5) },
        ];
        let rules: Vec<EffectRule> = selectors
            .iter()
            .enumerate()
            .map(|(i, selector)| EffectRule {
                name: format!("rule-{i}"),
                vendor: Some(EffectPattern {
                    selector: *selector,
                    value: EffectValue::Any,
                    followed_by: None,
                    preceded_by: None,
                    occurrence: None,
                }),
                // Only the vendor side names the second read.
                replacement: (i != 4).then_some(EffectPattern {
                    selector: *selector,
                    value: EffectValue::Any,
                    followed_by: None,
                    preceded_by: None,
                    occurrence: None,
                }),
                disposition: EffectDisposition::Required,
                min_occurrences: 0,
                max_occurrences: 1,
                reason: "fixture".into(),
            })
            .collect();
        let events = [
            ExecutionEvent::Read {
                address: 0x10,
                width: 4,
                value: 0,
            },
            ExecutionEvent::Read {
                address: 0x14,
                width: 4,
                value: 0,
            },
            ExecutionEvent::Write {
                address: 0x10,
                width: 4,
                value: 1,
            },
            ExecutionEvent::DelayMicros { value: 5 },
            ExecutionEvent::DelayMicros { value: 6 },
            ExecutionEvent::Fence {
                predecessor: 3,
                successor: 3,
            },
        ];
        for replacement in [false, true] {
            let index = RuleIndex::new(&rules, replacement);
            for event in &events {
                let scanned: Vec<usize> = rules
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        r.pattern(replacement)
                            .is_some_and(|p| p.selector.matches(event))
                    })
                    .map(|(i, _)| i)
                    .collect();
                let indexed: Vec<usize> = index.candidates(event).collect();
                assert_eq!(indexed, scanned, "{event:?} on replacement {replacement}");
            }
        }
        // Delays with a value interleave with delays of any value.
        let index = RuleIndex::new(&rules, false);
        let delay = ExecutionEvent::DelayMicros { value: 5 };
        assert_eq!(index.candidates(&delay).collect::<Vec<_>>(), [0, 2, 5, 7]);
    }
}
