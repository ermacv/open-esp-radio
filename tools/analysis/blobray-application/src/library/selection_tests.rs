#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_analysis_riscv_lift::RiscvDecoder;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

fn fixture() -> Executable {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let mut symbols = BTreeMap::new();
    let mut sections = BTreeMap::new();
    for (name, words) in [
        ("leaf", vec![0x0250_0513u32, 0x0000_8067]),
        ("middle", vec![0x0000_0317, 0x0003_0067]), // tail leaf
        (
            "root",
            vec![
                0xff01_0113,
                0x0011_2623,
                0x0000_0097,
                0x0000_80e7,
                0x0015_0513,
                0x00c1_2083,
                0x0101_0113,
                0x0000_8067,
            ],
        ),
        ("unrelated", vec![0x07b0_0793, 0x0000_8067]),
    ] {
        let section = object.add_section(
            Vec::new(),
            format!(".text.{name}").into_bytes(),
            SectionKind::Text,
        );
        let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        object.append_section_data(section, &bytes, 4);
        let symbol = object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: 0,
            size: bytes.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        });
        sections.insert(name, section);
        symbols.insert(name, symbol);
    }
    for (caller, offset, callee) in [("root", 8, "middle"), ("middle", 0, "leaf")] {
        object
            .add_relocation(
                sections[caller],
                Relocation {
                    offset,
                    symbol: symbols[callee],
                    addend: 0,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_RISCV_CALL_PLT,
                    },
                },
            )
            .expect("call relocation");
    }
    Executable::new(object.write().expect("synthetic object"))
}

#[derive(Default)]
struct Decoder(RefCell<BTreeSet<u32>>);
impl FunctionDecoder for Decoder {
    fn identity(&self) -> &'static str {
        RiscvDecoder.identity()
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        if let Some(word) = bytes.get(..4).and_then(|bytes| bytes.try_into().ok()) {
            self.0.borrow_mut().insert(u32::from_le_bytes(word));
        }
        RiscvDecoder.decode(bytes)
    }
    fn unsupported_flow(&self, bytes: &[u8]) -> UnsupportedFlow {
        RiscvDecoder.unsupported_flow(bytes)
    }
    fn reference(
        &self,
        relocation: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        c: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        RiscvDecoder.reference(relocation, all, section, c)
    }
}
impl FunctionSemantics for Decoder {
    fn semantic_identity(&self) -> &'static str {
        RiscvDecoder.semantic_identity()
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        RiscvDecoder.lift(bytes)
    }
    fn value_relocation(
        &self,
        relocation: &FunctionRelocation,
        operation: SemanticOp,
    ) -> ValueRelocation {
        RiscvDecoder.value_relocation(relocation, operation)
    }
}

fn records(
    inputs: &[Executable],
    names: &[String],
    summaries: bool,
    decoder: &Decoder,
) -> BTreeMap<Vec<u8>, Vec<FunctionRecord>> {
    let memory = WorkingMemory::new(64 << 20).expect("fixture memory");
    let mut out = BTreeMap::new();
    analyze_library(
        inputs,
        Assumptions {
            functions: names,
            abi: Some(CallAbi::RiscvInteger),
            call_summaries: summaries,
            ..Assumptions::default()
        },
        decoder,
        &memory,
        &mut || Ok(()),
        &mut |outcome, _| {
            match outcome {
                LibraryOutcome::Analyzed(a) => {
                    out.insert(
                        a.function.name.clone().expect("fixture name"),
                        a.records.to_vec(),
                    );
                }
                LibraryOutcome::Blocked { error, .. } => return Err(error.clone()),
                LibraryOutcome::Gap { reason, .. } => panic!("fixture gap: {reason}"),
            }
            Ok(())
        },
    )
    .expect("library selection");
    assert_eq!(memory.used(), 0);
    out
}

#[test]
fn selected_records_equal_full_records_and_only_transitive_callees_are_analyzed() {
    let inputs = [fixture()];
    for summaries in [false, true] {
        let full_decoder = Decoder::default();
        let full = records(&inputs, &[], summaries, &full_decoder);
        let decoder = Decoder::default();
        let selected = records(&inputs, &["root".into()], summaries, &decoder);
        assert_eq!(selected.len(), 1);
        assert_eq!(
            serde_json::to_vec(&selected[b"root".as_slice()]).expect("selected records"),
            serde_json::to_vec(&full[b"root".as_slice()]).expect("full records")
        );
        let decoded = decoder.0.borrow();
        assert!(
            !decoded.contains(&0x07b0_0793),
            "unrelated function is never decoded"
        );
        assert_eq!(
            decoded.contains(&0x0250_0513),
            summaries,
            "transitive leaf is needed only for summaries"
        );
        if summaries {
            assert!(selected[b"root".as_slice()].iter().any(|record| matches!(
                record,
                FunctionRecord::ReturnValue {
                    low: AbstractValue::Constant { value: 38 },
                    ..
                }
            )));
        }
    }
}

#[test]
fn a_missing_name_decodes_no_function() {
    let decoder = Decoder::default();
    assert!(records(&[fixture()], &["missing".into()], true, &decoder).is_empty());
    assert!(decoder.0.borrow().is_empty());
}
