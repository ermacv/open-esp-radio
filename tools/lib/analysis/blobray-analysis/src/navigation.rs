//! Bounded indexes over one saved function. No storage or target-selection authority.
use oer_riscv_model::*;
pub struct MemoryObservation<'a> {
    pub record: u64,
    pub offset: u64,
    pub access: MemoryKind,
    pub width: u8,
    pub address: &'a AbstractValue,
    pub value: Option<&'a AbstractValue>,
}
pub struct Facts<'a, 'm> {
    records: &'a [FunctionRecord],
    expressions: AdmittedVec<'m, &'a Expression>,
    references: AdmittedVec<'m, &'a ReferenceTarget>,
    inputs: AdmittedVec<'m, (u64, &'a [AbstractValue], u64)>,
    transfers: AdmittedVec<'m, (u64, u64, &'a AbstractValue, bool)>,
}
fn integrity(s: &str) -> Error {
    Error::new(ErrorCode::Integrity, s)
}
impl<'a, 'm> Facts<'a, 'm> {
    /// Borrow an already validated expression by its exact saved ID.
    pub(crate) fn value_expression(&self, value: &AbstractValue) -> Option<&'a Expression> {
        if let AbstractValue::Expression { id } = value {
            self.expressions.get(*id as usize).copied()
        } else {
            None
        }
    }
    pub fn new(
        records: &'a [FunctionRecord],
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut out = Self {
            records,
            expressions: AdmittedVec::new(memory),
            references: AdmittedVec::new(memory),
            inputs: AdmittedVec::new(memory),
            transfers: AdmittedVec::new(memory),
        };
        let mut instructions = AdmittedVec::new(memory);
        for (record, r) in records.iter().enumerate() {
            c.checkpoint(1)?;
            match r {
                FunctionRecord::Instruction { offset, .. } => {
                    instructions.push(*offset, c.position())?
                }
                FunctionRecord::Expression { id, expression, .. } => {
                    if *id as usize != out.expressions.len() {
                        return Err(integrity("noncanonical navigation expression IDs"));
                    }
                    let earlier = |v: &AbstractValue| !matches!(v,AbstractValue::Expression { id: other } if other >= id);
                    if !match expression {
                        Expression::Integer { left, right, .. } => earlier(left) && earlier(right),
                        Expression::Load { address, .. } => earlier(address),
                        _ => true,
                    } {
                        return Err(integrity("forward or cyclic navigation expression"));
                    }
                    out.expressions.push(expression, c.position())?;
                }
                FunctionRecord::Reference { target, .. } => {
                    out.references.push(target, c.position())?
                }
                FunctionRecord::CallInputs { offset, registers } => out
                    .inputs
                    .push((*offset, registers, record as u64), c.position())?,
                FunctionRecord::Transfer {
                    offset,
                    target,
                    call,
                } => out
                    .transfers
                    .push((*offset, record as u64, target, *call), c.position())?,
                _ => (),
            }
        }
        c.checkpoint(records.len() as u64 * (records.len().max(1).ilog2() as u64 + 1))?;
        out.references
            .sort_unstable_by(|a, b| a.symbol.cmp(&b.symbol));
        instructions.sort_unstable();
        out.inputs.sort_unstable_by_key(|x| x.0);
        out.transfers.sort_unstable_by_key(|x| x.0);
        if instructions.windows(2).any(|w| w[0] == w[1])
            || out.inputs.windows(2).any(|w| w[0].0 == w[1].0)
            || out.transfers.windows(2).any(|w| w[0].0 == w[1].0)
            || out
                .references
                .windows(2)
                .any(|w| w[0].symbol == w[1].symbol && w[0] != w[1])
        {
            return Err(integrity("duplicate or inconsistent navigation facts"));
        }
        Ok(out)
    }
    pub fn reference(
        &self,
        symbol: &SymbolId,
        c: &mut dyn RunControl,
    ) -> Result<Option<&ReferenceTarget>> {
        c.checkpoint(self.references.len().max(1).ilog2() as u64 + 1)?;
        Ok(self
            .references
            .binary_search_by(|r| r.symbol.cmp(symbol))
            .ok()
            .map(|i| self.references[i]))
    }
    #[cfg(test)]
    pub(crate) fn expression(&self, id: u32) -> Result<&Expression> {
        self.expressions
            .get(id as usize)
            .copied()
            .ok_or_else(|| integrity("expression ID is absent"))
    }
    pub fn accesses(
        &self,
        c: &mut dyn RunControl,
        emit: &mut dyn FnMut(MemoryObservation<'a>, &mut dyn RunControl) -> Result<()>,
    ) -> Result<()> {
        for (record, r) in self.records.iter().enumerate() {
            c.checkpoint(1)?;
            let FunctionRecord::MemoryAccess {
                offset,
                access,
                width,
                address,
                value,
                ..
            } = r
            else {
                continue;
            };
            emit(
                MemoryObservation {
                    record: record as u64,
                    offset: *offset,
                    access: *access,
                    width: *width,
                    address,
                    value: value.as_ref(),
                },
                c,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_expression_graphs_and_duplicate_inputs_fail_before_delivery() {
        let memory = WorkingMemory::new(8192).unwrap();
        for records in [
            vec![FunctionRecord::Expression {
                id: 0,
                offset: 0,
                expression: Expression::Load {
                    address: AbstractValue::Expression { id: 0 },
                    width: 4,
                    signed: false,
                },
            }],
            vec![FunctionRecord::Expression {
                id: 1,
                offset: 0,
                expression: Expression::EntryRegister { register: 10 },
            }],
            vec![
                FunctionRecord::CallInputs {
                    offset: 0,
                    registers: vec![],
                },
                FunctionRecord::CallInputs {
                    offset: 0,
                    registers: vec![],
                },
            ],
        ] {
            assert!(
                matches!(Facts::new(&records,&memory,&mut || Ok(())),Err(e) if e.code==ErrorCode::Integrity)
            );
            assert_eq!(memory.used(), 0);
        }
        let memory = WorkingMemory::new(1).unwrap();
        let records = [FunctionRecord::Expression {
            id: 0,
            offset: 0,
            expression: Expression::EntryRegister { register: 10 },
        }];
        assert!(
            matches!(Facts::new(&records,&memory,&mut || Ok(())),Err(e) if e.code==ErrorCode::ResourceLimited)
        );
        assert_eq!(memory.used(), 0);
    }
}
