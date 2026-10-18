use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};

#[allow(dead_code)]
pub fn elf() -> Vec<u8> {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &[0x13, 0, 0, 0, 0x67, 0x80, 0, 0], 4);
    for (name, scope, weak) in [
        (b"same".to_vec(), SymbolScope::Linkage, false),
        (b"local\xff".to_vec(), SymbolScope::Compilation, false),
        (b"weak".to_vec(), SymbolScope::Linkage, true),
    ] {
        object.add_symbol(Symbol {
            name,
            value: 0,
            size: 8,
            kind: SymbolKind::Text,
            scope,
            weak,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
    }
    let external = object.add_symbol(Symbol {
        name: b"external".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Unknown,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object.add_common_symbol(
        Symbol {
            name: b"common".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Common,
            flags: SymbolFlags::None,
        },
        16,
        4,
    );
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: external,
                addend: 7,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

#[allow(dead_code)]
pub fn archive(members: &[(&[u8], &[u8])], thin: bool) -> Vec<u8> {
    let mut result = if thin {
        b"!<thin>\n".to_vec()
    } else {
        b"!<arch>\n".to_vec()
    };
    for (name, bytes) in members {
        assert!(name.len() < 16);
        let mut header = [b' '; 60];
        header[..name.len()].copy_from_slice(name);
        header[name.len()] = b'/';
        let fields = format!(
            "{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            0,
            0,
            0,
            "644",
            bytes.len()
        );
        header[16..].copy_from_slice(fields.as_bytes());
        result.extend_from_slice(&header);
        if !thin {
            result.extend_from_slice(bytes);
            if bytes.len() % 2 != 0 {
                result.push(b'\n');
            }
        }
    }
    result
}

/// Add a physical DYNSYM with reversed global entries. The name is deliberately
/// absent: table identity comes from ELF headers, never a conventional name.
#[allow(dead_code)]
pub fn dynamic_symbols(mut bytes: Vec<u8>, only: bool) -> Vec<u8> {
    fn word(b: &[u8], at: usize) -> usize {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize
    }
    let shoff = word(&bytes, 32);
    let count = u16::from_le_bytes(bytes[48..50].try_into().unwrap()) as usize;
    assert_eq!(u16::from_le_bytes(bytes[46..48].try_into().unwrap()), 40);
    let mut headers = bytes[shoff..shoff + count * 40].to_vec();
    let static_index = (1..count)
        .find(|i| word(&headers, i * 40 + 4) == 2)
        .unwrap();
    let mut dynamic = headers[static_index * 40..(static_index + 1) * 40].to_vec();
    let offset = word(&dynamic, 16);
    let size = word(&dynamic, 20);
    assert_eq!(size % 16, 0);
    let locals = word(&dynamic, 28);
    let mut symbols = bytes[offset..offset + locals * 16].to_vec();
    for symbol in bytes[offset + locals * 16..offset + size]
        .chunks_exact(16)
        .rev()
    {
        symbols.extend_from_slice(symbol);
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    dynamic[0..4].copy_from_slice(&0u32.to_le_bytes());
    dynamic[4..8].copy_from_slice(&11u32.to_le_bytes());
    dynamic[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&symbols);
    if only {
        headers[static_index * 40 + 4..static_index * 40 + 8].copy_from_slice(&1u32.to_le_bytes());
    }
    let new_shoff = bytes.len() as u32;
    bytes[32..36].copy_from_slice(&new_shoff.to_le_bytes());
    bytes[48..50].copy_from_slice(&((count + 1) as u16).to_le_bytes());
    bytes.extend_from_slice(&headers);
    bytes.extend_from_slice(&dynamic);
    bytes
}
/// A static RV32 executable whose one loadable segment at 0x1000 holds `code`.
#[allow(dead_code)]
pub fn executable(code: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; 256];
    bytes[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 243), (40, 52), (42, 32), (44, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (20, 1u32),
        (24, 0x1000),
        (28, 52),
        (52, 1),
        (56, 256),
        (60, 0x1000),
        (64, 0x1000),
        (68, (code.len() * 4) as u32),
        (72, (code.len() * 4) as u32),
        (76, 5),
        (80, 4),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for op in code {
        bytes.extend_from_slice(&op.to_le_bytes());
    }
    bytes
}
/// `executable(code)` with `.text`, `.symtab` and `.strtab` naming global functions.
#[allow(dead_code)]
pub fn executable_with_symbols(code: &[u32], symbols: &[(&str, u32, u32)]) -> Vec<u8> {
    let put = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    let mut b = executable(code);
    let text = (256, (code.len() * 4) as u32);
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 16];
    for (name, address, size) in symbols {
        put(&mut symtab, strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        put(&mut symtab, *address);
        put(&mut symtab, *size);
        symtab.extend_from_slice(&[0x12, 0]); // STB_GLOBAL | STT_FUNC, default visibility
        symtab.extend_from_slice(&1u16.to_le_bytes()); // .text
    }
    let shstrtab = b"\0.text\0.symtab\0.strtab\0.shstrtab\0";
    let place = |b: &mut Vec<u8>, bytes: &[u8]| {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let offset = b.len() as u32;
        b.extend_from_slice(bytes);
        (offset, bytes.len() as u32)
    };
    let symtab = place(&mut b, &symtab);
    let strtab = place(&mut b, &strtab);
    let shstrtab = place(&mut b, shstrtab);
    let headers = place(&mut b, &[0; 40]).0;
    // name, type, flags, address, offset, size, link, info, align, entsize
    for header in [
        [1, 1, 6, 0x1000, text.0, text.1, 0, 0, 4, 0],
        [7, 2, 0, 0, symtab.0, symtab.1, 3, 1, 4, 16],
        [15, 3, 0, 0, strtab.0, strtab.1, 0, 0, 1, 0],
        [23, 3, 0, 0, shstrtab.0, shstrtab.1, 0, 0, 1, 0],
    ] {
        for v in header {
            put(&mut b, v);
        }
    }
    b[32..36].copy_from_slice(&headers.to_le_bytes());
    b[46..48].copy_from_slice(&40u16.to_le_bytes());
    b[48..50].copy_from_slice(&5u16.to_le_bytes());
    b[50..52].copy_from_slice(&4u16.to_le_bytes());
    b
}
