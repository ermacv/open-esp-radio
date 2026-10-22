#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::{StoredScan, Targets};
use crate::in_process::Executable;
use oer_analysis_riscv_model::{Error, ErrorCode, Result, RunControl, WorkingMemory};
fn executable(code: &[u32]) -> Vec<u8> {
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

// (name, address, size, ELF type, ELF section).
type Sym<'a> = (&'a str, u32, u32, u8, u16);

fn image(base: u32, code: &[u32], symbols: &[Sym<'_>], tables: bool) -> Vec<u8> {
    let put = |b: &mut Vec<u8>, value: u32| b.extend_from_slice(&value.to_le_bytes());
    let place = |b: &mut Vec<u8>, bytes: &[u8]| {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let offset = b.len() as u32;
        b.extend_from_slice(bytes);
        (offset, bytes.len() as u32)
    };
    let mut b = executable(code);
    for offset in [24, 60, 64] {
        b[offset..offset + 4].copy_from_slice(&base.to_le_bytes());
    }
    let mut headers = vec![
        [0u32; 10],
        [1, 1, 6, base, 256, (code.len() * 4) as u32, 0, 0, 4, 0],
    ];
    let mut names = b"\0.text\0".to_vec();
    if tables {
        for (name, flags) in [(".rodata", 2), (".data", 3)] {
            let name_offset = names.len() as u32;
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            let data = place(
                &mut b,
                &[0, 0, 0, 0, 0x34, 0x23, 0, 0, 0xff, 0xff, 0xff, 0xff],
            );
            headers.push([
                name_offset,
                1,
                flags,
                base + data.0 - 256,
                data.0,
                data.1,
                0,
                0,
                4,
                0,
            ]);
        }
    }
    let loaded = b.len() as u32 - 256;
    b[68..72].copy_from_slice(&loaded.to_le_bytes());
    b[72..76].copy_from_slice(&loaded.to_le_bytes());
    b[76..80].copy_from_slice(&7u32.to_le_bytes());
    let mut strings = vec![0];
    let mut syms = vec![0; 16];
    let mut all = symbols.to_vec();
    if tables {
        all.push(("ro_callbacks", headers[2][3], 12, 1, 2));
        all.push(("rw_callbacks", headers[3][3], 12, 1, 3));
    }
    for (name, address, size, kind, section) in all {
        put(&mut syms, strings.len() as u32);
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
        put(&mut syms, address);
        put(&mut syms, size);
        syms.extend_from_slice(&[0x10 | kind, 0]);
        syms.extend_from_slice(&section.to_le_bytes());
    }
    let symtab = place(&mut b, &syms);
    let strtab = place(&mut b, &strings);
    let symname = names.len() as u32;
    names.extend_from_slice(b".symtab\0");
    let strname = names.len() as u32;
    names.extend_from_slice(b".strtab\0");
    let shname = names.len() as u32;
    names.extend_from_slice(b".shstrtab\0");
    let shstr = place(&mut b, &names);
    let link = headers.len() as u32 + 1;
    headers.extend([
        [symname, 2, 0, 0, symtab.0, symtab.1, link, 1, 4, 16],
        [strname, 3, 0, 0, strtab.0, strtab.1, 0, 0, 1, 0],
        [shname, 3, 0, 0, shstr.0, shstr.1, 0, 0, 1, 0],
    ]);
    let shoff = place(&mut b, &[]).0;
    b[32..36].copy_from_slice(&shoff.to_le_bytes());
    b[46..48].copy_from_slice(&40u16.to_le_bytes());
    b[48..50].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    b[50..52].copy_from_slice(&((headers.len() - 1) as u16).to_le_bytes());
    for header in headers {
        for value in header {
            put(&mut b, value);
        }
    }
    b
}

struct Work(u64);
impl RunControl for Work {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(units)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "test budget"))?;
        Ok(())
    }
}

#[test]
fn data_scan_resource_failure_is_an_error_even_after_candidates_were_found() {
    let input = Executable::new(image(
        0x1000,
        &[0x00008067],
        &[("entry", 0x1000, 4, 2, 1), ("target", 0x2334, 0, 0, 0xfff1)],
        true,
    ));
    let memory = WorkingMemory::new(1 << 20).unwrap();
    let targets = Targets::capture(
        std::slice::from_ref(&input),
        &["target".into()],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let mut complete = StoredScan::new(&memory, &mut || Ok(())).unwrap();
    let mut meter = Work(u64::MAX);
    complete
        .scan_data(std::slice::from_ref(&input), &targets, &memory, &mut meter)
        .unwrap();
    let count = u64::MAX - meter.0;
    assert_eq!(complete.references.len(), 2);
    drop(complete);
    // Refuse on the last work checkpoint: earlier findings cannot turn failure into success.
    let mut scan = StoredScan::new(&memory, &mut || Ok(())).unwrap();
    let error = scan
        .scan_data(
            std::slice::from_ref(&input),
            &targets,
            &memory,
            &mut Work(count - 1),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert!(!scan.references.is_empty());
    // A held budget leaves too little memory for the exported section.
    let _held = memory
        .reserve((1 << 20) - memory.used() - 8, Default::default())
        .unwrap();
    let error = scan
        .scan_data(&[input], &targets, &memory, &mut || Ok(()))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
}
