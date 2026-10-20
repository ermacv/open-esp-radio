use super::*;

/// A two-byte relocated call followed by an unsupported instruction.
struct Isa;
impl FunctionDecoder for Isa {
    fn identity(&self) -> &'static str {
        "noreturn-test"
    }

    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        (bytes[0] == 0).then(|| DecodedOp {
            length: 2,
            text: String::new(),
            flow: InstructionFlow::Jump {
                displacement: 0,
                link: true,
            },
        })
    }

    fn reference(
        &self,
        raw: &FunctionRelocation,
        _: &[FunctionRelocation],
        _: u32,
        _: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        Ok(NormalizedReference {
            kind: ReferenceKind::Branch,
            target: raw.target.clone(),
            addend: raw.addend,
            paired: None,
            known: true,
        })
    }
}
impl FunctionSemantics for Isa {
    fn semantic_identity(&self) -> &'static str {
        "noreturn-test"
    }
    fn lift(&self, _: &[u8]) -> SemanticOp {
        SemanticOp::Link { dest: 1 }
    }
    fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
        ValueRelocation::Ignore
    }
}

struct Proof(bool);
impl CallBehavior for Proof {
    fn noreturn(&self, callee: Callee<'_>) -> bool {
        self.0 && callee == Callee::Symbol(b"callee")
    }
}

#[derive(Default)]
struct Records(Vec<FunctionRecord>);
impl FunctionSink for Records {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone());
        Ok(())
    }
}

#[test]
fn an_object_call_uses_explicit_behavior_independently_of_unknown_result_values() {
    // Only the unique global callee at addend zero is a concrete function.
    for (proof, addend, binding, count) in [
        (true, 0, 1, 1),
        (false, 0, 1, 1),
        (true, 2, 1, 1),
        (true, 0, 0, 1),
        (true, 0, 1, 2),
    ] {
        let target = std::sync::Arc::new(ReferenceTarget {
            binding,
            definition: SymbolDefinition::Undefined,
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"object"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 2,
                index: 1,
            },
            name: b"callee".to_vec(),
            section: None,
            offset: 0,
            symbol_type: 2,
        });
        let raw: Vec<_> = (0..count)
            .map(|index| FunctionRelocation {
                section: 3,
                index,
                offset: 0,
                relocation_type: 17,
                addend: Some(addend),
                target: target.clone(),
            })
            .collect();
        let memory = WorkingMemory::new(1024 * 1024).unwrap();
        let references = PreparedReferences::new(&raw, 1, &Isa, &memory, &mut || Ok(())).unwrap();
        let mut records = Records::default();
        let summary = analyze(
            FunctionInput {
                traps: crate::TrapSemantics::Unknown,
                image: None,
                section: 1,
                extent: CodeRange {
                    start: 0,
                    length: 4,
                },
                bytes: &[0, 0, 1, 0],
                relocations: &references,
                data_ranges: &[],
                jumps: &[],
                returns: &NoReturns,
                call_behavior: &Proof(proof),
            },
            &Isa,
            &memory,
            &mut || Ok(()),
            &mut records,
        )
        .unwrap();
        let noreturn = proof && addend == 0 && binding == 1 && count == 1;
        assert_eq!(
            summary.coverage.decoding, noreturn,
            "{proof} {addend} {binding} {count}"
        );
        assert!(records.0.iter().any(|record| matches!(
            record,
            FunctionRecord::Edge {
                from: 0,
                relation: EdgeKind::Call,
                ..
            }
        )));
        assert_eq!(
            records.0.iter().any(|record| matches!(
                record,
                FunctionRecord::Edge {
                    from: 0,
                    relation: EdgeKind::PossibleContinuation,
                    ..
                }
            )),
            !noreturn
        );
    }
}
