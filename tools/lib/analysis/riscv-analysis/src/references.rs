//! Section-scoped normalization. ISA owns pairing; consumers use bounded indexes.
use oer_riscv_model::*;
use std::ops::Deref;
pub struct PreparedReferences<'a> {
    raw: Vec<FunctionRelocation>,
    normalized: Vec<NormalizedReference>,
    symbols: Vec<Option<usize>>,
    pairs: Vec<Option<usize>>,
    _capacity: Option<MemoryReservation<'a>>,
}
fn vector<T>(count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "reference index allocation refused",
        )
    })?;
    Ok(v)
}
impl<'a> PreparedReferences<'a> {
    pub fn empty() -> Self {
        Self {
            raw: Vec::new(),
            normalized: Vec::new(),
            symbols: Vec::new(),
            pairs: Vec::new(),
            _capacity: None,
        }
    }
    pub fn new(
        raw: &[FunctionRelocation],
        section: u32,
        decoder: &dyn FunctionDecoder,
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        if raw.windows(2).any(|w| w[0].offset > w[1].offset) {
            return Err(Error::new(
                ErrorCode::Integrity,
                "relocation table is not sorted by offset",
            ));
        }
        let n = raw.len();
        let bytes = n
            .checked_mul(
                std::mem::size_of::<FunctionRelocation>()
                    + std::mem::size_of::<NormalizedReference>()
                    + 2 * std::mem::size_of::<Option<usize>>()
                    + 2 * std::mem::size_of::<usize>(),
            )
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "reference capacity overflow"))?;
        let capacity = memory.reserve(bytes as u64, c.position())?;
        let mut symbols = vector(n)?;
        let mut physical = vector(n)?;
        symbols.extend(0..n);
        physical.extend(0..n);
        c.checkpoint(
            (n as u64)
                .saturating_mul(u64::from(usize::BITS - n.leading_zeros()))
                .saturating_mul(2),
        )?;
        symbols.sort_unstable_by(|&a, &b| {
            raw[a]
                .target
                .symbol
                .cmp(&raw[b].target.symbol)
                .then(a.cmp(&b))
        });
        physical.sort_unstable_by_key(|&i| (raw[i].section, raw[i].index));
        let mut result = Self {
            raw: vector(n)?,
            normalized: vector(n)?,
            symbols: vector(n)?,
            pairs: vector(n)?,
            _capacity: Some(capacity),
        };
        result.raw.extend_from_slice(raw);
        for r in raw {
            c.checkpoint(1)?;
            let normalized = decoder.reference(r, raw, section, c)?;
            let at = symbols.partition_point(|&i| raw[i].target.symbol < normalized.target.symbol);
            result.symbols.push(
                symbols
                    .get(at)
                    .copied()
                    .filter(|&i| raw[i].target.symbol == normalized.target.symbol),
            );
            result.pairs.push(normalized.paired.and_then(|key| {
                physical
                    .binary_search_by_key(&key, |&i| (raw[i].section, raw[i].index))
                    .ok()
                    .map(|at| physical[at])
            }));
            result.normalized.push(normalized);
        }
        Ok(result)
    }
    pub fn first_at(&self, offset: u64) -> usize {
        self.raw.partition_point(|r| r.offset < offset)
    }
    pub fn normalized(&self, index: usize) -> &NormalizedReference {
        &self.normalized[index]
    }
    pub fn symbol(&self, index: usize) -> Option<usize> {
        self.symbols[index]
    }
    pub fn pair(&self, index: usize) -> Option<usize> {
        self.pairs[index]
    }
}
impl Deref for PreparedReferences<'_> {
    type Target = [FunctionRelocation];
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
