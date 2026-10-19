//! Fixed-depth expression observations, never physical field inference.
use crate::navigation::Facts;
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;

fn constant(v: &AbstractValue) -> Option<u32> {
    if let AbstractValue::Constant { value } = v {
        Some(*value)
    } else {
        None
    }
}
fn masked(expression: &Expression) -> Option<(&AbstractValue, u32)> {
    if let Expression::Integer {
        op: IntegerOp::And,
        left,
        right,
    } = expression
    {
        constant(right)
            .map(|mask| (left, mask))
            .or_else(|| constant(left).map(|mask| (right, mask)))
    } else {
        None
    }
}
fn width_mask(width: u8) -> Option<u32> {
    match width {
        1 => Some(0xff),
        2 => Some(0xffff),
        4 => Some(u32::MAX),
        _ => None,
    }
}
/// Recognize `(load >> shift) & mask` or `load & mask` and retain source bit positions.
/// Signed extension bits beyond the load width are excluded.
pub fn read_mask<'a>(
    facts: &Facts<'a, '_>,
    expression: &'a Expression,
) -> Option<(&'a AbstractValue, u8, RegisterMask)> {
    let (mut value, bits) = masked(expression)?;
    let mut shift = 0;
    if let Some(Expression::Integer {
        op: IntegerOp::Shr | IntegerOp::Sar,
        left,
        right,
    }) = facts.value_expression(value)
    {
        shift = constant(right)? & 31;
        value = left;
    }
    if let Some(Expression::Load { address, width, .. }) = facts.value_expression(value) {
        let bits = bits.wrapping_shl(shift) & width_mask(*width)?;
        (bits != 0).then_some((
            address,
            *width,
            RegisterMask {
                kind: RegisterMaskKind::ReadSelection,
                bits,
            },
        ))
    } else {
        None
    }
}

/// Recognize a same-address/width load-preserve-OR store. The result includes
/// every bit the replacement may set; unknown replacement conservatively covers all bits.
pub fn write_mask(
    facts: &Facts<'_, '_>,
    address: &AbstractValue,
    width: u8,
    value: &AbstractValue,
) -> Option<RegisterMask> {
    let mut expression = facts.value_expression(value)?;
    if let Some((value, mask)) = masked(expression)
        && mask == width_mask(width)?
    {
        expression = facts.value_expression(value)?;
    }
    let Expression::Integer {
        op: IntegerOp::Or,
        left,
        right,
    } = expression
    else {
        return None;
    };
    for (preserved, replacement) in [(left, right), (right, left)] {
        let Some(e) = facts.value_expression(preserved) else {
            continue;
        };
        let Some((load, preserve)) = masked(e) else {
            continue;
        };
        if let Some(Expression::Load {
            address: original,
            width: original_width,
            ..
        }) = facts.value_expression(load)
            && address == original
            && width == *original_width
        {
            return Some(RegisterMask {
                kind: RegisterMaskKind::WriteReplacement,
                bits: (!preserve | constant(replacement).unwrap_or(u32::MAX)) & width_mask(width)?,
            });
        }
    }
    None
}

/// One bit of a stored value: fixed, an entry register's bit, a loaded
/// value's bit, or unknown. A loaded bit names its load expression's id, so
/// two loads of one address (two reads of a register) are never one source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bit<'a> {
    Zero,
    One,
    Entry(u8, u8),
    Load(u32, &'a AbstractValue, u8, u8),
    Unknown,
}

fn constant_bits<'a>(value: u32) -> [Bit<'a>; 32] {
    std::array::from_fn(|i| {
        if value >> i & 1 == 1 {
            Bit::One
        } else {
            Bit::Zero
        }
    })
}

fn and<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::Zero, _) | (_, Bit::Zero) => Bit::Zero,
        (Bit::One, x) | (x, Bit::One) => x,
        (x, y) if x == y && x != Bit::Unknown => x,
        _ => Bit::Unknown,
    }
}

fn or<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::One, _) | (_, Bit::One) => Bit::One,
        (Bit::Zero, x) | (x, Bit::Zero) => x,
        (x, y) if x == y && x != Bit::Unknown => x,
        _ => Bit::Unknown,
    }
}

fn not(a: Bit<'_>) -> Bit<'_> {
    match a {
        Bit::Zero => Bit::One,
        Bit::One => Bit::Zero,
        _ => Bit::Unknown,
    }
}

fn xor<'a>(a: Bit<'a>, b: Bit<'a>) -> Bit<'a> {
    match (a, b) {
        (Bit::Zero, x) | (x, Bit::Zero) => x,
        (Bit::One, x) | (x, Bit::One) => not(x),
        _ => Bit::Unknown,
    }
}

/// `left + right` while no bit position can carry; from the first position
/// where both may be set, every higher bit is unknown.
fn add<'a>(left: [Bit<'a>; 32], right: [Bit<'a>; 32]) -> [Bit<'a>; 32] {
    let mut carry = false;
    std::array::from_fn(|i| {
        if carry {
            return Bit::Unknown;
        }
        match (left[i], right[i]) {
            (Bit::Zero, x) | (x, Bit::Zero) => x,
            _ => {
                carry = true;
                Bit::Unknown
            }
        }
    })
}

fn shift_left(bits: [Bit<'_>; 32], by: u32) -> [Bit<'_>; 32] {
    std::array::from_fn(|i| {
        if (i as u32) < by {
            Bit::Zero
        } else {
            bits[i - by as usize]
        }
    })
}

/// Bit-level sources of stored values over one function's expressions.
///
/// Expressions form a DAG whose shared nodes the analysis consolidates, so each
/// node is evaluated once per function and memoized by its dense id; every
/// newly evaluated node is charged one work unit and the memo reserves its
/// capacity in working memory before it grows, so the walk stays inside the
/// caller's work budget, deadline and memory.
pub struct StoredBitsEvaluator<'f, 'a, 'm> {
    facts: &'f Facts<'a, 'm>,
    memory: &'m WorkingMemory,
    /// Each evaluated expression's bits, by expression id.
    memo: AdmittedVec<'m, Option<[Bit<'a>; 32]>>,
}

impl<'f, 'a, 'm> StoredBitsEvaluator<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            facts,
            memory,
            memo: AdmittedVec::new(memory),
        }
    }

    /// The bits of `value`. A leaf is read directly; an expression is
    /// evaluated with every node it reaches computed exactly once per
    /// function, in post order on an admitted stack: an operand's id is
    /// always lower than its node's, so the walk needs no depth bound.
    fn bits(&mut self, value: &'a AbstractValue, c: &mut dyn RunControl) -> Result<[Bit<'a>; 32]> {
        let AbstractValue::Expression { id } = value else {
            return Ok(self.operand(value, u32::MAX));
        };
        if self.facts.value_expression(value).is_none() {
            return Ok([Bit::Unknown; 32]);
        }
        let mut stack = AdmittedVec::new(self.memory);
        stack.push(*id, c.position())?;
        while let Some(&top) = stack.last() {
            if self.memoized(top).is_some() {
                stack.pop();
                continue;
            }
            let Some(expression) = self
                .facts
                .value_expression(&AbstractValue::Expression { id: top })
            else {
                stack.pop();
                continue;
            };
            let mut pending = false;
            if let Expression::Integer { left, right, .. } = expression {
                for operand in [left, right] {
                    if let AbstractValue::Expression { id: operand } = operand
                        && *operand < top
                        && self.memoized(*operand).is_none()
                        && self
                            .facts
                            .value_expression(&AbstractValue::Expression { id: *operand })
                            .is_some()
                    {
                        stack.push(*operand, c.position())?;
                        pending = true;
                    }
                }
            }
            if pending {
                continue;
            }
            c.checkpoint(1)?;
            let result = match expression {
                Expression::EntryRegister { register } => {
                    std::array::from_fn(|i| Bit::Entry(*register, i as u8))
                }
                Expression::Load {
                    address,
                    width,
                    signed,
                } => {
                    let loaded = u32::from(*width) * 8;
                    std::array::from_fn(|i| {
                        let i = i as u32;
                        if i < loaded {
                            Bit::Load(top, address, *width, i as u8)
                        } else if *signed && loaded != 0 {
                            Bit::Load(top, address, *width, (loaded - 1) as u8)
                        } else {
                            Bit::Zero
                        }
                    })
                }
                Expression::Integer { op, left, right } => {
                    let l = self.operand(left, top);
                    let r = self.operand(right, top);
                    let by = constant(right).map(|k| k & 31);
                    match op {
                        IntegerOp::And => std::array::from_fn(|i| and(l[i], r[i])),
                        IntegerOp::AndNot => std::array::from_fn(|i| and(l[i], not(r[i]))),
                        IntegerOp::Or => std::array::from_fn(|i| or(l[i], r[i])),
                        IntegerOp::Xor => std::array::from_fn(|i| xor(l[i], r[i])),
                        IntegerOp::Shl => match by {
                            Some(by) => shift_left(l, by),
                            None => [Bit::Unknown; 32],
                        },
                        IntegerOp::Shr | IntegerOp::Sar => match by {
                            Some(by) => std::array::from_fn(|i| {
                                let from = i as u32 + by;
                                if from < 32 {
                                    l[from as usize]
                                } else if *op == IntegerOp::Sar {
                                    l[31]
                                } else {
                                    Bit::Zero
                                }
                            }),
                            None => [Bit::Unknown; 32],
                        },
                        IntegerOp::Add => add(l, r),
                        IntegerOp::Sub if r.iter().all(|bit| *bit == Bit::Zero) => l,
                        IntegerOp::ShiftAdd1 => add(shift_left(l, 1), r),
                        IntegerOp::ShiftAdd2 => add(shift_left(l, 2), r),
                        IntegerOp::ShiftAdd3 => add(shift_left(l, 3), r),
                        _ => [Bit::Unknown; 32],
                    }
                }
                Expression::CallResult { .. } => [Bit::Unknown; 32],
            };
            while self.memo.len() <= top as usize {
                self.memo.push(None, c.position())?;
            }
            self.memo[top as usize] = Some(result);
            stack.pop();
        }
        Ok(self.memoized(*id).unwrap_or([Bit::Unknown; 32]))
    }

    fn memoized(&self, id: u32) -> Option<[Bit<'a>; 32]> {
        self.memo.get(id as usize).copied().flatten()
    }

    /// An operand of node `node`: a leaf's bits, or an already evaluated
    /// earlier expression's; anything else is unknown.
    fn operand(&self, value: &'a AbstractValue, node: u32) -> [Bit<'a>; 32] {
        match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                constant_bits(*value)
            }
            AbstractValue::Expression { id } if *id < node => {
                self.memoized(*id).unwrap_or([Bit::Unknown; 32])
            }
            _ => [Bit::Unknown; 32],
        }
    }
}

/// Operator nodes an indexed address is followed through; deeper sums are
/// left unresolved, never truncated into a different progression.
const INDEXED_DEPTH: u8 = 16;

/// `constant + coefficient * term`, every operation modulo 2^32, with at most
/// one term: a value the linear walk cannot split further. Terms are equal
/// when they are the same leaf or the same expression node.
#[derive(Clone, Copy)]
struct Linear<'a> {
    constant: u32,
    term: Option<(&'a AbstractValue, u32)>,
}

/// What one expression node decomposes to, memoized by its id: a
/// decomposition (or `None` when two different terms meet) and the levels it
/// needs, or the depth it could not be decomposed within.
#[derive(Clone, Copy)]
enum Decomposed<'a> {
    Known(Option<Linear<'a>>, u8),
    Deeper(u8),
}

impl<'a> Linear<'a> {
    fn scale(self, by: u32) -> Self {
        Self {
            constant: self.constant.wrapping_mul(by),
            term: self
                .term
                .map(|(term, coefficient)| (term, coefficient.wrapping_mul(by))),
        }
    }

    /// The sum, or `None` when the operands carry two different terms.
    fn add(self, other: Self) -> Option<Self> {
        let term = match (self.term, other.term) {
            (None, term) | (term, None) => term,
            (Some((a, x)), Some((b, y))) if a == b => Some((a, x.wrapping_add(y))),
            _ => return None,
        };
        Some(Self {
            constant: self.constant.wrapping_add(other.constant),
            term,
        })
    }
}

/// The decomposition of `value` within `depth` levels, memoized in the
/// caller's `memo` (one per function) by each expression node's dense id in
/// admitted working memory: an expression is a DAG whose nodes several sums
/// share, so a node reached again within no more levels costs nothing. A node
/// that did not decompose within its levels is decomposed again only when it
/// is reached with more, so at most `INDEXED_DEPTH + 1` times.
fn linear<'a, 'm>(
    facts: &Facts<'a, '_>,
    value: &'a AbstractValue,
    depth: u8,
    memo: &mut AdmittedVec<'m, Option<Decomposed<'a>>>,
    c: &mut dyn RunControl,
) -> Result<Decomposed<'a>> {
    if let Some(constant) = exact_address(value) {
        return Ok(Decomposed::Known(
            Some(Linear {
                constant,
                term: None,
            }),
            0,
        ));
    }
    let term = Linear {
        constant: 0,
        term: Some((value, 1)),
    };
    let (AbstractValue::Expression { id }, Some(Expression::Integer { op, left, right })) =
        (value, facts.value_expression(value))
    else {
        return Ok(Decomposed::Known(Some(term), 0));
    };
    let slot = *id as usize;
    match memo.get(slot).copied().flatten() {
        Some(known @ Decomposed::Known(..)) => return Ok(known),
        Some(Decomposed::Deeper(within)) if depth <= within => {
            return Ok(Decomposed::Deeper(depth));
        }
        _ => {}
    }
    let outcome = if depth == 0 {
        Decomposed::Deeper(0)
    } else {
        c.checkpoint(1)?;
        // The operands' decompositions within one level less, with the
        // levels they need; `None` when one is deeper.
        let mut height = 0u8;
        let mut operand = |v: &'a AbstractValue,
                           memo: &mut AdmittedVec<'m, Option<Decomposed<'a>>>,
                           c: &mut dyn RunControl|
         -> Result<Option<Option<Linear<'a>>>> {
            Ok(match linear(facts, v, depth - 1, memo, c)? {
                Decomposed::Known(result, needs) if needs < depth => {
                    height = height.max(needs);
                    Some(result)
                }
                _ => None,
            })
        };
        let result = match op {
            IntegerOp::Add | IntegerOp::Sub => {
                match (operand(left, memo, c)?, operand(right, memo, c)?) {
                    (Some(l), Some(r)) => Some(match (l, r) {
                        (Some(l), Some(r)) if *op == IntegerOp::Add => l.add(r),
                        (Some(l), Some(r)) => l.add(r.scale(u32::MAX)),
                        _ => None,
                    }),
                    _ => None,
                }
            }
            IntegerOp::Shl => match constant(right) {
                Some(by) => operand(left, memo, c)?.map(|l| l.map(|l| l.scale(1 << (by & 31)))),
                None => Some(Some(term)),
            },
            IntegerOp::Mul => match (constant(left), constant(right)) {
                (_, Some(by)) => operand(left, memo, c)?.map(|l| l.map(|l| l.scale(by))),
                (Some(by), _) => operand(right, memo, c)?.map(|r| r.map(|r| r.scale(by))),
                _ => Some(Some(term)),
            },
            IntegerOp::ShiftAdd1 | IntegerOp::ShiftAdd2 | IntegerOp::ShiftAdd3 => {
                let by = match op {
                    IntegerOp::ShiftAdd1 => 2,
                    IntegerOp::ShiftAdd2 => 4,
                    _ => 8,
                };
                match (operand(left, memo, c)?, operand(right, memo, c)?) {
                    (Some(l), Some(r)) => Some(match (l, r) {
                        (Some(l), Some(r)) => l.scale(by).add(r),
                        _ => None,
                    }),
                    _ => None,
                }
            }
            _ => Some(Some(term)),
        };
        match result {
            Some(result) => Decomposed::Known(result, height + 1),
            None => Decomposed::Deeper(depth),
        }
    };
    if memo.len() <= slot {
        c.checkpoint((slot + 1 - memo.len()) as u64)?;
        while memo.len() <= slot {
            memo.push(None, c.position())?;
        }
    }
    memo[slot] = Some(outcome);
    Ok(outcome)
}

/// The values an index expression is proven to stay below: `x & mask`, a
/// zero-extending byte or halfword load, `x %u n` and `x >> k`; `None` when
/// any 32-bit value is possible.
fn index_count(facts: &Facts<'_, '_>, index: &AbstractValue) -> Option<u32> {
    match facts.value_expression(index)? {
        Expression::Integer { op, left, right } => match op {
            IntegerOp::And => constant(right)
                .or_else(|| constant(left))
                .and_then(|mask| mask.checked_add(1)),
            IntegerOp::Remu => constant(right).filter(|n| *n != 0),
            IntegerOp::Shr => constant(right)
                .map(|by| by & 31)
                .filter(|by| *by != 0)
                .map(|by| 1 << (32 - by)),
            IntegerOp::Lt | IntegerOp::Ltu => Some(2),
            _ => None,
        },
        Expression::Load {
            width: 1,
            signed: false,
            ..
        } => Some(1 << 8),
        Expression::Load {
            width: 2,
            signed: false,
            ..
        } => Some(1 << 16),
        _ => None,
    }
}

/// The progressions of one function's unresolved addresses, over one memo
/// for the whole function: an expression node is decomposed once, whichever
/// address reaches it, and again only when reached with more levels than it
/// failed within (at most `INDEXED_DEPTH + 1` times). The memo lives in
/// admitted working memory, and filling it up to a new id is charged too.
pub struct IndexedAddresses<'f, 'a, 'm> {
    facts: &'f Facts<'a, 'm>,
    memo: AdmittedVec<'m, Option<Decomposed<'a>>>,
}

impl<'f, 'a, 'm> IndexedAddresses<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            facts,
            memo: AdmittedVec::new(memory),
        }
    }

    /// The progression an unresolved `address` selects from when the
    /// analysis computes it as a constant plus a nonzero multiple of one
    /// unknown index, through additions, subtractions, constant shifts and
    /// multiplications and `shNadd`. Any other shape, two different unknowns
    /// or a sum deeper than [`INDEXED_DEPTH`] is `None`: the address stays
    /// unresolved.
    pub fn address(
        &mut self,
        address: &'a AbstractValue,
        c: &mut dyn RunControl,
    ) -> Result<Option<IndexedAddress>> {
        let facts = self.facts;
        if facts.value_expression(address).is_none() {
            return Ok(None);
        }
        let Decomposed::Known(
            Some(Linear {
                constant,
                term: Some((index, coefficient)),
            }),
            _,
        ) = linear(facts, address, INDEXED_DEPTH, &mut self.memo, c)?
        else {
            return Ok(None);
        };
        let count = index_count(facts, index);
        // An unbounded index with a unit step reaches every address: that is a
        // pointer plus an offset, not an array, and says nothing.
        if coefficient == 0 || (count.is_none() && matches!(coefficient, 1 | u32::MAX)) {
            return Ok(None);
        }
        Ok(Some(IndexedAddress {
            base: constant,
            stride: coefficient as i32,
            count,
        }))
    }
}

fn exact_address(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            Some(*value)
        }
        _ => None,
    }
}

/// Where each run of the `width`-byte value stored at `address` comes from,
/// low run first: fixed bits, a function entry register's bits (an argument
/// for `a0`..`a7`), a loaded value's bits (with `same_word` for the stored word
/// itself) or unknown. Each bit follows bitwise logic, constant shifts and
/// carry-free additions of the analysis' expressions; anything else is
/// unknown, never guessed.
pub fn stored_bits<'a>(
    evaluator: &mut StoredBitsEvaluator<'_, 'a, '_>,
    address: &AbstractValue,
    width: u8,
    value: &'a AbstractValue,
    c: &mut dyn RunControl,
) -> Result<Vec<StoredBits>> {
    let Some(mask) = width_mask(width) else {
        return Ok(Vec::new());
    };
    let value = evaluator.bits(value, c)?;
    let count = mask.count_ones() as usize;
    let mut runs: Vec<StoredBits> = Vec::new();
    let mut i = 0;
    while i < count {
        let start = i;
        let first = value[i];
        i += 1;
        let next = |previous: Bit<'a>, bit: Bit<'a>| match (previous, bit) {
            (Bit::Zero | Bit::One, Bit::Zero | Bit::One) => true,
            (Bit::Entry(r, b), Bit::Entry(s, c)) => r == s && c == b + 1,
            (Bit::Load(e, _, _, b), Bit::Load(f, _, _, c)) => e == f && c == b + 1,
            (Bit::Unknown, Bit::Unknown) => true,
            _ => false,
        };
        while i < count && next(value[i - 1], value[i]) {
            i += 1;
        }
        let source = match first {
            Bit::Zero | Bit::One => StoredBitsSource::Constant {
                value: (start..i).fold(0, |acc, bit| {
                    acc | u32::from(value[bit] == Bit::One) << (bit - start)
                }),
            },
            Bit::Entry(register, low) => StoredBitsSource::EntryRegister { register, low },
            Bit::Load(_, loaded, loaded_width, low) => StoredBitsSource::Load {
                address: exact_address(loaded),
                width: loaded_width,
                low,
                same_word: loaded == address && loaded_width == width,
            },
            Bit::Unknown => StoredBitsSource::Unknown,
        };
        runs.push(StoredBits {
            low: start as u8,
            width: (i - start) as u8,
            source,
        });
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(id: u32) -> AbstractValue {
        AbstractValue::Expression { id }
    }
    fn number(value: u32) -> AbstractValue {
        AbstractValue::Constant { value }
    }
    fn records(expressions: Vec<Expression>) -> Vec<FunctionRecord> {
        expressions
            .into_iter()
            .enumerate()
            .map(|(id, expression)| FunctionRecord::Expression {
                id: id as u32,
                offset: id as u64 * 4,
                expression,
            })
            .collect()
    }
    #[test]
    fn masks_preserve_source_bit_positions_and_never_shrink_unknown_replacement() {
        let memory = WorkingMemory::new(65536).unwrap();
        let records = records(vec![
            Expression::Load {
                address: number(0x20000),
                width: 2,
                signed: true,
            },
            Expression::Integer {
                op: IntegerOp::Shr,
                left: value(0),
                right: number(4),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(1),
                right: number(15),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(0),
                right: number(!15),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: AbstractValue::Unknown,
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: number(0x100),
            },
        ]);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let Expression::Integer { .. } = facts
            .expression(2, &mut || Ok(()))
            .expect("lookup succeeds")
            .expect("expression 2 exists")
        else {
            panic!()
        };
        let (_, width, mask) = read_mask(
            &facts,
            facts
                .expression(2, &mut || Ok(()))
                .expect("lookup succeeds")
                .expect("expression 2 exists"),
        )
        .unwrap();
        assert_eq!((width, mask.bits), (2, 0xf0));
        assert_eq!(
            write_mask(&facts, &number(0x20000), 2, &value(4))
                .unwrap()
                .bits,
            0xffff
        );
        assert_eq!(
            write_mask(&facts, &number(0x20000), 2, &value(5))
                .unwrap()
                .bits,
            0x10f
        );
        assert!(write_mask(&facts, &number(0x20001), 2, &value(5)).is_none());
        assert!(write_mask(&facts, &number(0x20000), 4, &value(5)).is_none());
    }
    #[test]
    fn stored_bits_name_fields_kept_bits_constants_and_unknowns() {
        let memory = WorkingMemory::new(65536).unwrap();
        let word = number(0x2010_713c);
        let records = records(vec![
            Expression::Load {
                address: word.clone(),
                width: 4,
                signed: false,
            },
            Expression::EntryRegister { register: 10 },
            Expression::Integer {
                op: IntegerOp::Shl,
                left: value(1),
                right: number(18),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(2),
                right: number(0x01fc_0000),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(0),
                right: number(0xfe03_ffff),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: value(4),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(4),
                right: number(0x1b << 18),
            },
            Expression::Integer {
                op: IntegerOp::Mul,
                left: value(1),
                right: number(3),
            },
            Expression::EntryRegister { register: 11 },
            Expression::Integer {
                op: IntegerOp::Add,
                left: value(1),
                right: value(8),
            },
        ]);
        let (field, fixed, product, sum, small) =
            (value(5), value(6), value(7), value(9), number(0x1ff));
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
        macro_rules! stored_bits {
            ($address:expr, $width:expr, $value:expr) => {
                super::stored_bits(&mut evaluator, $address, $width, $value, &mut || Ok(()))
                    .unwrap()
            };
        }
        let kept = |low, width| StoredBits {
            low,
            width,
            source: StoredBitsSource::Load {
                address: Some(0x2010_713c),
                width: 4,
                low,
                same_word: true,
            },
        };
        assert_eq!(
            stored_bits!(&word, 4, &field),
            [
                kept(0, 18),
                StoredBits {
                    low: 18,
                    width: 7,
                    source: StoredBitsSource::EntryRegister {
                        register: 10,
                        low: 0
                    },
                },
                kept(25, 7),
            ],
            "the argument's low seven bits replace bits 24:18; the rest are kept"
        );
        assert_eq!(
            stored_bits!(&word, 4, &fixed)[1],
            StoredBits {
                low: 18,
                width: 7,
                source: StoredBitsSource::Constant { value: 0x1b },
            }
        );
        let elsewhere = number(0x2010_7094);
        assert!(matches!(
            stored_bits!(&elsewhere, 4, &field)[0].source,
            StoredBitsSource::Load {
                same_word: false,
                address: Some(0x2010_713c),
                ..
            }
        ));
        assert_eq!(
            stored_bits!(&word, 4, &product),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Unknown,
            }],
            "a product is never decomposed"
        );
        assert_eq!(
            stored_bits!(&word, 4, &sum),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Unknown,
            }],
            "two register values can carry from bit zero"
        );
        assert_eq!(
            stored_bits!(&word, 1, &small),
            [StoredBits {
                low: 0,
                width: 8,
                source: StoredBitsSource::Constant { value: 0xff },
            }],
            "only the access width is described"
        );
    }
    #[test]
    fn two_loads_of_one_address_are_two_sources() {
        let memory = WorkingMemory::new(65536).unwrap();
        let word = number(0x6000_0000);
        let load = || Expression::Load {
            address: word.clone(),
            width: 4,
            signed: false,
        };
        let records = records(vec![
            load(),
            load(),
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(0),
                right: value(1),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(0),
                right: number(0xff),
            },
            Expression::Integer {
                op: IntegerOp::And,
                left: value(1),
                right: number(0xff00),
            },
            Expression::Integer {
                op: IntegerOp::Or,
                left: value(3),
                right: value(4),
            },
        ]);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let (both, bytes) = (value(2), value(5));
        let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
        let mut stored =
            |value| super::stored_bits(&mut evaluator, &word, 4, value, &mut || Ok(())).unwrap();
        assert_eq!(
            stored(&both),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Unknown,
            }],
            "the OR of two reads of one register is neither read"
        );
        let read = |low, width| StoredBits {
            low,
            width,
            source: StoredBitsSource::Load {
                address: Some(0x6000_0000),
                width: 4,
                low,
                same_word: true,
            },
        };
        assert_eq!(
            stored(&bytes),
            [
                read(0, 8),
                read(8, 8),
                StoredBits {
                    low: 16,
                    width: 16,
                    source: StoredBitsSource::Constant { value: 0 },
                },
            ],
            "bytes of two reads stay two runs"
        );
    }
    #[test]
    fn a_shared_expression_dag_is_evaluated_once_per_node_inside_the_budget() {
        let memory = WorkingMemory::new(65536).unwrap();
        // x0 = a0; x(i+1) = x(i) + x(i): one node per step, 2^steps paths.
        let chain = |steps: u32| {
            let mut expressions = vec![Expression::EntryRegister { register: 10 }];
            for id in 0..steps {
                expressions.push(Expression::Integer {
                    op: IntegerOp::Add,
                    left: value(id),
                    right: value(id),
                });
            }
            records(expressions)
        };
        let visits = |records: &[FunctionRecord], top: &AbstractValue| {
            let facts = Facts::new(records, &memory, &mut || Ok(())).unwrap();
            let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
            let mut visits = 0_u32;
            let runs = stored_bits(&mut evaluator, &number(0x2010_702c), 4, top, &mut || {
                visits += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(
                runs,
                [StoredBits {
                    low: 0,
                    width: 32,
                    source: StoredBitsSource::Unknown,
                }],
                "a register added to itself can carry from bit zero"
            );
            visits
        };
        let (short, short_top) = (chain(20), value(20));
        assert_eq!(
            visits(&short, &short_top),
            21,
            "each of the 21 nodes is charged once"
        );
        let (long, long_top) = (chain(40), value(40));
        assert_eq!(
            visits(&long, &long_top),
            41,
            "every one of the 41 nodes is evaluated exactly once, at any depth"
        );
        let facts = Facts::new(&long, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
        let mut exhausted = || Err(Error::new(ErrorCode::ResourceLimited, "work budget"));
        assert!(
            stored_bits(
                &mut evaluator,
                &number(0x2010_702c),
                4,
                &long_top,
                &mut exhausted
            )
            .is_err(),
            "the walk stops when the budget does"
        );
    }
    #[test]
    fn the_memo_reserves_working_memory_before_it_grows() {
        let records = records(vec![
            Expression::EntryRegister { register: 10 },
            Expression::Integer {
                op: IntegerOp::Add,
                left: value(0),
                right: value(0),
            },
        ]);
        let top = value(1);
        // Enough for the expression index, not for eight memo entries of
        // thirty-two bit sources each.
        let memo_bytes = 8 * std::mem::size_of::<Option<(u8, [Bit<'_>; 32])>>() as u64;
        let memory = WorkingMemory::new(memo_bytes - 1).unwrap();
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
        let error = stored_bits(
            &mut evaluator,
            &number(0x2010_702c),
            4,
            &top,
            &mut || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
    #[test]
    fn a_node_reached_again_by_a_later_store_costs_nothing() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut expressions = vec![Expression::EntryRegister { register: 10 }];
        for id in 0..30 {
            expressions.push(Expression::Integer {
                op: IntegerOp::Or,
                left: value(id),
                right: number(1 << (id % 32)),
            });
        }
        let records = records(expressions);
        let (deep, middle) = (value(30), value(15));
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut evaluator = StoredBitsEvaluator::new(&facts, &memory);
        let mut charged = 0_u32;
        stored_bits(&mut evaluator, &number(0), 4, &deep, &mut || {
            charged += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(charged, 31);
        let mut again = 0_u32;
        stored_bits(&mut evaluator, &number(0), 4, &middle, &mut || {
            again += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(again, 0, "node 15 was evaluated by the first store's walk");
    }

    /// The indexed progression of `expressions`' last node.
    fn indexed(expressions: Vec<Expression>) -> Option<IndexedAddress> {
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let last = value(expressions.len() as u32 - 1);
        let records = records(expressions);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        IndexedAddresses::new(&facts, &memory)
            .address(&last, &mut || Ok(()))
            .unwrap()
    }
    fn integer(op: IntegerOp, left: AbstractValue, right: AbstractValue) -> Expression {
        Expression::Integer { op, left, right }
    }
    fn argument(register: u8) -> Expression {
        Expression::EntryRegister { register }
    }

    #[test]
    fn an_address_of_one_scaled_index_is_an_indexed_progression() {
        // coex_hw_timer_set: `(index + 0x2010f40) << 4`.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Add, value(0), number(0x0201_0f40)),
                integer(IntegerOp::Shl, value(1), number(4)),
                integer(IntegerOp::Add, value(2), number(4)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_f404,
                stride: 0x10,
                count: None,
            })
        );
        // mac_tx_set_mplen: a descending queue vector, `index * -0x7c + base`.
        assert_eq!(
            indexed(vec![
                argument(11),
                integer(IntegerOp::Mul, value(0), number(0xffff_ff84)),
                integer(IntegerOp::Add, value(1), number(0x2010_54fc)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_54fc,
                stride: -0x7c,
                count: None,
            })
        );
        // A masked index bounds the progression; `sh2add` scales it.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::And, value(0), number(7)),
                integer(IntegerOp::ShiftAdd2, value(1), number(0x2010_4000)),
                integer(IntegerOp::Sub, value(2), number(8)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_3ff8,
                stride: 4,
                count: Some(8),
            })
        );
        // The same index twice is still one index.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Shl, value(0), number(3)),
                integer(IntegerOp::Add, value(1), value(0)),
                integer(IntegerOp::Add, value(2), number(0x2010_0000)),
            ])
            .map(|indexed| indexed.stride),
            Some(9)
        );
    }

    #[test]
    fn a_pointer_offset_or_two_indices_stay_unresolved() {
        // A loaded pointer plus a field offset: any address, not an array.
        assert_eq!(
            indexed(vec![
                Expression::Load {
                    address: number(0x3fc0_0000),
                    width: 4,
                    signed: false,
                },
                integer(IntegerOp::Add, value(0), number(8)),
            ]),
            None
        );
        // Two different unknowns.
        assert_eq!(
            indexed(vec![
                argument(10),
                argument(11),
                integer(IntegerOp::Shl, value(0), number(4)),
                integer(IntegerOp::Add, value(2), value(1)),
            ]),
            None
        );
        // An index that cancels out leaves no progression.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Shl, value(0), number(2)),
                integer(IntegerOp::Sub, value(1), value(1)),
                integer(IntegerOp::Add, value(2), number(0x2010_0000)),
            ]),
            None
        );
        // A byte index bounds a unit stride, which is then an array again.
        assert_eq!(
            indexed(vec![
                Expression::Load {
                    address: number(0x3fc0_0000),
                    width: 1,
                    signed: false,
                },
                integer(IntegerOp::Add, value(0), number(0x2010_0000)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_0000,
                stride: 1,
                count: Some(256),
            })
        );
    }

    #[test]
    fn a_sum_deeper_than_the_bound_stays_unresolved_and_is_charged() {
        let mut expressions = vec![argument(10), integer(IntegerOp::Shl, value(0), number(2))];
        for id in 1..=u32::from(INDEXED_DEPTH) + 1 {
            expressions.push(integer(IntegerOp::Add, value(id), number(4)));
        }
        assert_eq!(indexed(expressions), None);
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let records = records(vec![
            argument(10),
            integer(IntegerOp::Shl, value(0), number(2)),
            integer(IntegerOp::Add, value(1), number(0x2010_0000)),
        ]);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let refused = IndexedAddresses::new(&facts, &memory).address(&value(2), &mut || {
            Err(Error::new(ErrorCode::ResourceLimited, "work budget"))
        });
        assert_eq!(refused.unwrap_err().code, ErrorCode::ResourceLimited);
    }

    #[test]
    fn a_shared_node_is_decomposed_once_however_many_paths_reach_it() {
        // x0 = a0 & 7; x(i+1) = x(i) + x(i); the address is x14 + base: 2^14
        // paths reach x0, and each node is still charged once.
        let mut expressions = vec![argument(10), integer(IntegerOp::And, value(0), number(7))];
        for id in 1..15 {
            expressions.push(integer(IntegerOp::Add, value(id), value(id)));
        }
        expressions.push(integer(
            IntegerOp::Add,
            value(expressions.len() as u32 - 1),
            number(0x2010_0000),
        ));
        let nodes = expressions.len() as u64;
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let last = value(expressions.len() as u32 - 1);
        let records = records(expressions);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut checkpoints = 0u64;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        let mut addresses = IndexedAddresses::new(&facts, &memory);
        assert_eq!(
            addresses.address(&last, &mut count).unwrap(),
            Some(IndexedAddress {
                base: 0x2010_0000,
                stride: 1 << 14,
                count: Some(8),
            })
        );
        assert!(
            checkpoints <= 2 * nodes,
            "{checkpoints} checkpoints for {nodes} nodes"
        );
        // A second address of the function reuses the memo: no node is
        // decomposed again.
        let before = checkpoints;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        addresses.address(&last, &mut count).unwrap();
        assert_eq!(checkpoints, before);
    }
}
