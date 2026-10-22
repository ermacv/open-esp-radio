#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};

fn object(instruction: u32, size: u64) -> Executable {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    object.append_section_data(text, &instruction.to_le_bytes(), 4);
    object.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    Executable::new(object.write().unwrap())
}

#[test]
fn all_three_queries_count_blocked_functions_gaps_and_partial_causes_identically() {
    let inputs = [
        object(0x0000_8067, 4),                         // ret: complete
        object(0, 4),                                   // undecodable instruction: partial
        object(0x0000_8067, 0),                         // no extent: blocked
        Executable::new(b"unsupported input".to_vec()), // explicit container/object gaps
    ];
    let memory = WorkingMemory::new(64 << 20).unwrap();
    let symbols = ["missing".into()];
    for call_summaries in [false, true] {
        let assumptions = Assumptions {
            call_summaries,
            ..Assumptions::default()
        };
        let mut blocks = [0; 3];
        let field = field_accesses(
            &inputs, assumptions, 148, Some(1), &oer_analysis_riscv_lift::RiscvDecoder,
            &memory, &mut || Ok(()), &mut |result, _| {
                if matches!(result, FunctionResult::Blocked { error, .. } if error.code == ErrorCode::NeedsExtent) { blocks[0] += 1; }
                Ok(())
            },
        ).unwrap();
        let arguments = call_arguments(
            &inputs, assumptions, &symbols, &oer_analysis_riscv_lift::RiscvDecoder,
            &memory, &mut || Ok(()), &mut |result, _| {
                if matches!(result, FunctionResult::Blocked { error, .. } if error.code == ErrorCode::NeedsExtent) { blocks[1] += 1; }
                Ok(())
            },
        ).unwrap();
        let references = callers(
            &inputs, assumptions, &symbols, &oer_analysis_riscv_lift::RiscvDecoder,
            &memory, &mut || Ok(()), &mut |result, _| {
                if matches!(result, CallerResult::Function(FunctionResult::Blocked { error, .. }) if error.code == ErrorCode::NeedsExtent) { blocks[2] += 1; }
                Ok(())
            },
        ).unwrap();
        assert_eq!(field.analysis, arguments);
        assert_eq!(arguments, references.analysis);
        assert_eq!(
            (
                arguments.functions,
                arguments.partial,
                arguments.blocked,
                arguments.gaps
            ),
            (2, 1, 1, 3)
        );
        assert_eq!(arguments.partial_causes.decoding, 1);
        assert_eq!(blocks, [1; 3]);
        assert_eq!(memory.used(), 0, "operation-local allocations are released");
    }
}
