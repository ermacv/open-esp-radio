//! Finite reviewed byte-field and conditional-control correspondence for exact captured entries.
use crate::*;
pub const MAX_LAYOUT_FIELDS: usize = 128;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutDomain {
    pub address: u32,
    pub length: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldLocation {
    pub domain: u16,
    pub offset: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutEndpoint {
    pub entry: CallEndpoint,
    pub domains: Vec<LayoutDomain>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutField {
    pub name: String,
    pub vendor: FieldLocation,
    pub replacement: FieldLocation,
    /// Same-width little-endian bytes; no truncation, sign conversion or pointer normalization.
    pub width: u8,
    pub count: u32,
    pub final_state: bool,
    pub timeline: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchLocation {
    pub site: u32,
    pub target: u32,
    pub fallthrough: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchPair {
    pub vendor: BranchLocation,
    pub replacement: BranchLocation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutProjection {
    pub vendor: LayoutEndpoint,
    pub replacement: LayoutEndpoint,
    pub fields: Vec<LayoutField>,
    pub branches: Vec<BranchPair>,
    pub applicability: String,
    pub reason: String,
}
/// A projection supplied with a comparison, with the digest of its canonical
/// encoding a relation selects it by.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProjection {
    pub id: ArtifactId,
    pub projection: LayoutProjection,
}
fn invalid() -> Error {
    Error::new(
        ErrorCode::InvalidRequest,
        "invalid reviewed layout projection",
    )
}
fn overlap(a: u32, al: u32, b: u32, bl: u32) -> bool {
    u64::from(a) < u64::from(b) + u64::from(bl) && u64::from(b) < u64::from(a) + u64::from(al)
}
impl LayoutField {
    pub fn byte_length(&self) -> Result<u32> {
        self.count
            .checked_mul(u32::from(self.width))
            .filter(|n| *n > 0 && *n <= MAX_OBSERVED_MEMORY_BYTES)
            .ok_or_else(invalid)
    }
}
impl LayoutEndpoint {
    pub fn address(&self, field: FieldLocation, length: u32) -> Result<u32> {
        let domain = self
            .domains
            .get(field.domain as usize)
            .ok_or_else(invalid)?;
        if u64::from(field.offset) + u64::from(length) > u64::from(domain.length) {
            return Err(invalid());
        }
        domain.address.checked_add(field.offset).ok_or_else(invalid)
    }
    fn validate(&self) -> Result<()> {
        self.entry.validate()?;
        if !matches!(self.entry.boundary, ReviewedCallBoundary::Code { .. })
            || self.domains.len() > MAX_LAYOUT_FIELDS
        {
            return Err(invalid());
        }
        for (i, d) in self.domains.iter().enumerate() {
            if d.length == 0
                || u64::from(d.address) + u64::from(d.length) >= u64::from(u32::MAX - 1)
                || self.domains[..i]
                    .iter()
                    .any(|p| overlap(d.address, d.length, p.address, p.length))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    fn allocated_bytes(&self) -> u64 {
        self.entry.allocated_bytes()
            + (self.domains.capacity() * std::mem::size_of::<LayoutDomain>()) as u64
    }
}
impl LayoutProjection {
    pub fn endpoint(&self, replacement: bool) -> &LayoutEndpoint {
        if replacement {
            &self.replacement
        } else {
            &self.vendor
        }
    }
    pub fn field_address(&self, field: &LayoutField, replacement: bool) -> Result<u32> {
        self.endpoint(replacement).address(
            if replacement {
                field.replacement
            } else {
                field.vendor
            },
            field.byte_length()?,
        )
    }
    pub fn validate(&self) -> Result<()> {
        self.vendor.validate()?;
        self.replacement.validate()?;
        if (self.fields.is_empty() && self.branches.is_empty())
            || self.fields.len() > MAX_LAYOUT_FIELDS
            || self.branches.len() > MAX_LAYOUT_FIELDS
            || self.applicability.trim().is_empty()
            || self.applicability.len() > 1024
            || self.reason.trim().is_empty()
            || self.reason.len() > 4096
        {
            return Err(invalid());
        }
        let mut total = 0u64;
        for (i, f) in self.fields.iter().enumerate() {
            total += u64::from(f.byte_length()?);
            if total > u64::from(MAX_OBSERVED_MEMORY_BYTES) {
                return Err(invalid());
            }
            if f.name.trim().is_empty()
                || f.name.len() > 128
                || !f.name.is_ascii()
                || !matches!(f.width, 1 | 2 | 4 | 8)
                || (!f.final_state && !f.timeline)
                || self.fields[..i].iter().any(|p| p.name == f.name)
            {
                return Err(invalid());
            }
            for side in [false, true] {
                let a = self.field_address(f, side)?;
                if !a.is_multiple_of(u32::from(f.width)) {
                    return Err(invalid());
                }
                for p in &self.fields[..i] {
                    if overlap(
                        a,
                        f.byte_length()?,
                        self.field_address(p, side)?,
                        p.byte_length()?,
                    ) {
                        return Err(invalid());
                    }
                }
            }
        }
        for side in [false, true] {
            let e = self.endpoint(side);
            // Every declared domain is meaningful, not an ignored layout declaration.
            for i in 0..e.domains.len() {
                if !self.fields.iter().any(|f| {
                    usize::from(if side {
                        f.replacement.domain
                    } else {
                        f.vendor.domain
                    }) == i
                }) {
                    return Err(invalid());
                }
            }
            for (i, p) in self.branches.iter().enumerate() {
                let b = if side { p.replacement } else { p.vendor };
                if b.site & 1 != 0 || b.site >= u32::MAX - 1 || b.target & 1 != 0
                    || b.fallthrough >= u32::MAX - 1
                    || !matches!(b.fallthrough.checked_sub(b.site),Some(2|4))
                    || self.branches[..i].iter().any(|p| if side {p.replacement.site} else {p.vendor.site} == b.site) {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    pub fn allocated_bytes(&self) -> u64 {
        self.vendor.allocated_bytes()
            + self.replacement.allocated_bytes()
            + (self.fields.capacity() * std::mem::size_of::<LayoutField>()) as u64
            + (self.branches.capacity() * std::mem::size_of::<BranchPair>()) as u64
            + self
                .fields
                .iter()
                .map(|f| f.name.capacity() as u64)
                .sum::<u64>()
            + self.applicability.capacity() as u64
            + self.reason.capacity() as u64
    }
    pub fn validate_use(&self, request: &ExecutionRequest, phase: usize) -> Result<()> {
        self.validate()?;
        let case = request.cases.get(phase).ok_or_else(invalid)?;
        let relation = case.relation.as_ref().ok_or_else(invalid)?;
        let right = request.replacement.as_ref().ok_or_else(invalid)?;
        let ri = case.replacement.as_ref().ok_or_else(invalid)?;
        if self.fields.iter().any(|f| f.timeline)
            && !(relation.events.timeline.reads
                || relation.events.timeline.writes
                || relation.events.timeline.atomics)
            || !self.branches.is_empty() && !relation.events.timeline.branches
        {
            return Err(invalid());
        }
        for (side, target, input) in [(false, &request.vendor, &case.vendor), (true, right, ri)] {
            let endpoint = &self.endpoint(side).entry;
            if !endpoint.in_target(target)
                || endpoint.address() != input.entry
                || target.abi != CallAbi::RiscvInteger
            {
                return Err(invalid());
            }
            for f in self.fields.iter().filter(|f| f.final_state) {
                let address = self.field_address(f, side)?;
                let selected = self.final_selection(input, f, side)?;
                if relation
                    .memory
                    .iter()
                    .any(|p| usize::from(if side { p.replacement } else { p.vendor }) == selected.0)
                {
                    return Err(invalid()); // an explicitly projected field cannot also silently take a physical relation
                }
                if address < input.observe_memory[selected.0].address {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    /// Exact borrowed snapshot location for every selected final field, including unchanged bytes.
    pub fn final_selection(
        &self,
        input: &Invocation,
        field: &LayoutField,
        side: bool,
    ) -> Result<(usize, u32)> {
        let address = self.field_address(field, side)?;
        let length = field.byte_length()?;
        input
            .observe_memory
            .iter()
            .enumerate()
            .find_map(|(i, s)| {
                (address >= s.address
                    && u64::from(address) + u64::from(length)
                        <= u64::from(s.address) + u64::from(s.length))
                .then(|| (i, address - s.address))
            })
            .ok_or_else(invalid)
    }
    /// Canonical field/offset identities; an unmapped selected effect is unknown, never omitted.
    pub fn memory_location(
        &self,
        transaction: MemoryTransaction,
        side: bool,
    ) -> Result<Option<(u16, u32)>> {
        let (address, length) = transaction.range();
        for (i, f) in self.fields.iter().enumerate().filter(|(_, f)| f.timeline) {
            let a = self.field_address(f, side)?;
            if address >= a
                && u64::from(address) + u64::from(length)
                    <= u64::from(a) + u64::from(f.byte_length()?)
            {
                return Ok(Some((i as u16, address - a)));
            }
        }
        Ok(None)
    }
    pub fn branch_location(&self, location: BranchLocation, side: bool) -> Option<u16> {
        self.branches
            .iter()
            .position(|p| if side { p.replacement } else { p.vendor } == location)
            .map(|i| i as u16)
    }
}

/// Resolve an explicitly selected immutable projection. Missing selection never falls back.
pub fn selected_projection<'a>(
    relation: Option<&ComparisonRelation>,
    projections: &'a [ResolvedProjection],
) -> Result<Option<&'a ResolvedProjection>> {
    let Some(id) = relation.and_then(|r| r.projection.as_ref()) else {
        return Ok(None);
    };
    let mut found = projections.iter().filter(|p| &p.id == id);
    let p = found
        .next()
        .ok_or_else(|| Error::new(ErrorCode::Integrity, "selected resolved projection missing"))?;
    if found.next().is_some() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "duplicate resolved projection",
        ));
    }
    Ok(Some(p))
}
