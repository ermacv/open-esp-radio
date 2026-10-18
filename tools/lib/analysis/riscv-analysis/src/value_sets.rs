//! Finite, canonical nonrecursive sets interned within one admitted analysis phase.
use super::values::Value;
use oer_riscv_model::*;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Set {
    pub values: [Value; MAX_VALUE_ALTERNATIVES],
    pub len: usize,
}
impl Set {
    fn empty() -> Self {
        Self {
            values: [Value::Unknown; MAX_VALUE_ALTERNATIVES],
            len: 0,
        }
    }
    fn insert(&mut self, value: Value) -> bool {
        match self.values[..self.len].binary_search(&value) {
            Ok(_) => true,
            Err(_) if self.len == MAX_VALUE_ALTERNATIVES => false,
            Err(at) => {
                self.values.copy_within(at..self.len, at + 1);
                self.values[at] = value;
                self.len += 1;
                true
            }
        }
    }
    fn hash(&self) -> usize {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        Hash::hash(self, &mut h);
        h.finish() as usize
    }
}
pub(super) struct Sets<'a> {
    memory: &'a WorkingMemory,
    values: AdmittedVec<'a, Set>,
    slots: AdmittedVec<'a, Option<u32>>,
}
impl<'a> Sets<'a> {
    pub fn new(memory: &'a WorkingMemory) -> Self {
        Self {
            memory,
            values: AdmittedVec::new(memory),
            slots: AdmittedVec::new(memory),
        }
    }
    pub fn get(&self, value: Value) -> Set {
        if let Value::Set(id) = value {
            self.values[id as usize]
        } else {
            let mut set = Set::empty();
            set.insert(value);
            set
        }
    }
    fn exact(value: Value) -> bool {
        matches!(
            value,
            Value::Constant(_)
                | Value::Image(_)
                | Value::Section(..)
                | Value::Symbol(..)
                | Value::Stack(_)
        )
    }
    fn intern(&mut self, set: Set, c: &mut dyn RunControl) -> Result<Value> {
        if set.len == 1 {
            return Ok(set.values[0]);
        }
        if self.slots.is_empty() || self.values.len() * 2 >= self.slots.len() {
            let size = self
                .slots
                .len()
                .checked_mul(2)
                .ok_or_else(|| {
                    Error::new(ErrorCode::ResourceLimited, "alternative index overflow")
                })?
                .max(16);
            let mut slots = AdmittedVec::new(self.memory);
            for _ in 0..size {
                c.checkpoint(1)?;
                slots.push(None, c.position())?;
            }
            for (id, old) in self.values.iter().enumerate() {
                let mut at = old.hash() & (size - 1);
                while slots[at].is_some() {
                    c.checkpoint(1)?;
                    at = (at + 1) & (size - 1);
                }
                slots[at] = Some(id as u32);
            }
            self.slots = slots;
        }
        let mut at = set.hash() & (self.slots.len() - 1);
        while let Some(id) = self.slots[at] {
            c.checkpoint(1)?;
            if self.values[id as usize] == set {
                return Ok(Value::Set(id));
            }
            at = (at + 1) & (self.slots.len() - 1);
        }
        let id = u32::try_from(self.values.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "alternative ID overflow"))?;
        self.values.push(set, c.position())?;
        self.slots[at] = Some(id);
        Ok(Value::Set(id))
    }
    pub fn join(&mut self, a: Value, b: Value, c: &mut dyn RunControl) -> Result<Value> {
        c.checkpoint(1)?;
        if a == b {
            return Ok(a);
        }
        if a == Value::Unknown || b == Value::Unknown {
            return Ok(Value::Unknown);
        }
        if a == Value::Widened || b == Value::Widened {
            return Ok(Value::Widened);
        }
        let mut out = Set::empty();
        for v in [a, b] {
            let values = self.get(v);
            for &v in &values.values[..values.len] {
                if !Self::exact(v) {
                    return Ok(Value::Unknown);
                }
                if !out.insert(v) {
                    return Ok(Value::Widened);
                }
            }
        }
        self.intern(out, c)
    }
    pub fn map(
        &mut self,
        v: Value,
        c: &mut dyn RunControl,
        mut f: impl FnMut(Value, &mut dyn RunControl) -> Result<Value>,
    ) -> Result<Value> {
        let values = self.get(v);
        let mut out = Set::empty();
        for &v in &values.values[..values.len] {
            c.checkpoint(1)?;
            let value = if v == Value::Widened { v } else { f(v, c)? };
            if values.len == 1 {
                return Ok(value);
            }
            if !Self::exact(value) {
                return Ok(value);
            }
            if !out.insert(value) {
                return Ok(Value::Widened);
            }
        }
        self.intern(out, c)
    }
    pub fn binary(
        &mut self,
        a: Value,
        b: Value,
        c: &mut dyn RunControl,
        f: impl Fn(Value, Value) -> Value,
    ) -> Result<Value> {
        if a == Value::Widened || b == Value::Widened {
            return Ok(Value::Widened);
        }
        let left = self.get(a);
        let right = self.get(b);
        if left.len == 1 && right.len == 1 {
            return Ok(f(a, b));
        }
        let mut out = Set::empty();
        for &a in &left.values[..left.len] {
            for &b in &right.values[..right.len] {
                c.checkpoint(1)?;
                let v = f(a, b);
                if !Self::exact(v) {
                    return Ok(v);
                }
                if !out.insert(v) {
                    return Ok(Value::Widened);
                }
            }
        }
        self.intern(out, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_join_is_canonical_bounded_and_releases_its_index() {
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        {
            let mut sets = Sets::new(&memory);
            let mut c = || Ok(());
            let a = sets
                .join(Value::Constant(1), Value::Constant(2), &mut c)
                .unwrap();
            let b = sets
                .join(Value::Constant(2), Value::Constant(1), &mut c)
                .unwrap();
            assert_eq!(a, b);
            assert_eq!(sets.join(a, a, &mut c).unwrap(), a);
            let bc = sets.join(b, Value::Constant(3), &mut c).unwrap();
            assert_eq!(sets.join(a, bc, &mut c).unwrap(), bc);
            assert_eq!(
                sets.join(bc, Value::Unknown, &mut c).unwrap(),
                Value::Unknown
            );
            let mut all = bc;
            for n in 4..=8 {
                all = sets.join(all, Value::Constant(n), &mut c).unwrap();
            }
            assert_eq!(sets.get(all).len, 8);
            assert_eq!(
                sets.join(all, Value::Constant(9), &mut c).unwrap(),
                Value::Widened
            );
            // Cartesian operations retain all outputs and collapse duplicates.
            let sum = sets
                .binary(a, b, &mut c, |a, b| match (a, b) {
                    (Value::Constant(a), Value::Constant(b)) => Value::Constant(a + b),
                    _ => unreachable!(),
                })
                .unwrap();
            let got = sets.get(sum);
            assert_eq!(
                &got.values[..got.len],
                &[Value::Constant(2), Value::Constant(3), Value::Constant(4)]
            );
            let mapped = sets.map(a, &mut c, |_, _| Ok(Value::Image(16))).unwrap();
            assert_eq!(mapped, Value::Image(16));
            assert!(memory.observation().reserved_bytes > 0);
        }
        assert_eq!(memory.observation().reserved_bytes, 0);
    }
    #[test]
    fn alternative_admission_and_cancellation_return_errors_without_leaks() {
        for bytes in [1, 1024 * 1024] {
            let memory = WorkingMemory::new(bytes).unwrap();
            {
                let mut sets = Sets::new(&memory);
                let mut left = 4u64;
                let result = sets.join(Value::Constant(1), Value::Constant(2), &mut || {
                    left = left.saturating_sub(1);
                    if left == 0 {
                        Err(Error::new(ErrorCode::Cancelled, "cancelled fixture"))
                    } else {
                        Ok(())
                    }
                });
                assert_eq!(
                    result.unwrap_err().code,
                    if bytes == 1 {
                        ErrorCode::ResourceLimited
                    } else {
                        ErrorCode::Cancelled
                    }
                );
            }
            assert_eq!(memory.observation().reserved_bytes, 0);
        }
    }
}
