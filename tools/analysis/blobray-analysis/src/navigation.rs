//! Blobray's canonical-record contract over the shared structural index.
use oer_analysis_riscv_model::*;
use std::ops::Deref;

/// Construction guarantees the earlier-expression DAG that Blobray's
/// expression evaluators require. Index storage and lookup belong to model.
pub struct Facts<'a, 'm>(FunctionIndex<'a, 'm>);

impl<'a, 'm> Facts<'a, 'm> {
    pub fn new(
        records: &'a [FunctionRecord],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        FunctionIndex::new(records, RecordPolicy::Canonical, memory, c).map(Self)
    }
}
impl<'a, 'm> Deref for Facts<'a, 'm> {
    type Target = FunctionIndex<'a, 'm>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobray_requires_canonical_records_before_interpretation() {
        let records = [FunctionRecord::Expression {
            id: 5,
            offset: 0,
            expression: Expression::EntryRegister { register: 10 },
        }];
        let memory = WorkingMemory::new(8192).unwrap();
        assert!(
            matches!(Facts::new(&records, &memory, &mut || Ok(())), Err(e) if e.code == ErrorCode::Integrity)
        );
        assert_eq!(memory.used(), 0);
        assert!(
            matches!(Facts::new(&[], &memory, &mut || Err(Error::new(ErrorCode::Cancelled, "stop"))), Err(e) if e.code == ErrorCode::Cancelled)
        );
        assert_eq!(memory.used(), 0);
    }
}
