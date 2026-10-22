//! End-to-end callers and argument queries over synthetic captured ELF inputs.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
#[allow(dead_code)]
mod support;
use oer_analysis_blobray_application::in_process::Executable;
use oer_analysis_blobray_application::references::{StoredScan, Targets};
use oer_analysis_riscv_model::{Error, ErrorCode, Result, RunControl, WorkingMemory};
use serde_json::{Value, json};
use std::process::Command;

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
    let mut b = support::executable(code);
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

fn invoke(command: &str, inputs: &[Vec<u8>], extra: &[&str]) -> std::process::Output {
    let temp = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blobray"));
    cmd.args(["--format", "json", command, "--symbol", "target"]);
    for (i, bytes) in inputs.iter().enumerate() {
        let path = temp.path().join(format!("{i}.elf"));
        std::fs::write(&path, bytes).unwrap();
        cmd.args(["--input", &format!("input{i}={}", path.display())]);
    }
    cmd.args(extra).output().unwrap()
}
fn query(command: &str, inputs: &[Vec<u8>]) -> Value {
    let output = invoke(command, inputs, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn sites(doc: &Value, field: &str) -> Vec<Value> {
    doc["callers"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|caller| caller[field].as_array().unwrap().clone())
        .collect()
}
const CALL: [u32; 4] = [0x00700513, 0x000022b7, 0x234280e7, 0x00008067]; // a0=7; lui t0,2; jalr ra,0x234(t0); ret

#[test]
fn provide_absolute_symbol_resolves_calls_and_their_arguments() {
    let input = image(
        0x1000,
        &CALL,
        &[
            ("entry", 0x1000, 16, 2, 1),
            ("target", 0x2234, 0, 0, 0xfff1),
        ],
        false,
    );
    let refs = sites(
        &query("callers", std::slice::from_ref(&input)),
        "references",
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["kind"], "transfer");
    assert_eq!(
        refs[0]["destination"],
        json!({"input":0,"address":0x2234,"name":"target"})
    );
    let args = sites(&query("call-arguments", &[input]), "sites");
    assert_eq!(args[0]["offset"], 0x1008);
    assert_eq!(
        args[0]["arguments"][0],
        json!({"kind":"constant","value":7})
    );
}

#[test]
fn rom_only_names_and_same_names_at_different_addresses_stay_separate() {
    let code = [0x000022b7, 0x234280e7, 0x000032b7, 0x234280e7, 0x00008067];
    let inputs = [
        image(0x1000, &code, &[("entry", 0x1000, 20, 2, 1)], false),
        image(0x2234, &[0x00008067], &[("target", 0x2234, 4, 2, 1)], false),
        image(0x3234, &[0x00008067], &[("target", 0x3234, 4, 2, 1)], false),
    ];
    for (command, field) in [("callers", "references"), ("call-arguments", "sites")] {
        let refs = sites(&query(command, &inputs), field);
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(refs[0]["destination"]["input"], 1);
        assert_eq!(refs[1]["destination"]["input"], 2);
        assert_eq!(refs[0]["destination"]["address"], 0x2234);
        assert_eq!(refs[1]["destination"]["address"], 0x3234);
    }
}

#[test]
fn a_tail_jump_is_an_exact_transfer() {
    let input = image(
        0x1000,
        &[0x000022b7, 0x23428067],
        &[("entry", 0x1000, 8, 2, 1), ("target", 0x2234, 0, 0, 0xfff1)],
        false,
    );
    let refs = sites(&query("callers", &[input]), "references");
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["kind"], "transfer");
}

#[test]
fn alternative_transfer_targets_are_explicit_may_calls() {
    // beqz a0,+12; lui t0,2; j +8; lui t0,3; jalr ra,0x234(t0); ret
    let input = image(
        0x1000,
        &[
            0x00050663, 0x000022b7, 0x0080006f, 0x000032b7, 0x234280e7, 0x00008067,
        ],
        &[
            ("entry", 0x1000, 24, 2, 1),
            ("target", 0x2234, 0, 0, 0xfff1),
        ],
        false,
    );
    for (command, field) in [("callers", "references"), ("call-arguments", "sites")] {
        let refs = sites(&query(command, std::slice::from_ref(&input)), field);
        assert_eq!(refs.len(), 1, "{refs:?}");
        assert_eq!(refs[0]["kind"], "transfer-may");
        assert_eq!(refs[0]["alternatives"], 2);
    }
}

#[test]
fn both_writable_and_read_only_tables_name_the_word_and_sized_owner() {
    let input = image(
        0x1000,
        &[0x00008067],
        &[("entry", 0x1000, 4, 2, 1), ("target", 0x2334, 0, 0, 0xfff1)],
        true,
    );
    let doc = query("callers", &[input]);
    let stored = doc["stored"].as_array().unwrap();
    assert_eq!(stored.len(), 2, "{doc}");
    for (i, name) in ["ro_callbacks", "rw_callbacks"].into_iter().enumerate() {
        assert_eq!(stored[i]["kind"], "stored-word");
        assert_eq!(stored[i]["owners"][0]["name"], name);
        assert_eq!(stored[i]["address"], 0x1008 + i * 12);
    }
    assert_eq!(doc["scan"]["sections"], 2);
    assert_eq!(doc["scan"]["words"], 6);
}

#[test]
fn a_materialized_pointer_stored_to_memory_is_a_candidate() {
    // lui t0,2; addi t0,t0,0x234; sw t0,0(a0); ret
    let input = image(
        0x1000,
        &[0x000022b7, 0x23428293, 0x00552023, 0x00008067],
        &[
            ("entry", 0x1000, 16, 2, 1),
            ("target", 0x2234, 0, 0, 0xfff1),
        ],
        false,
    );
    let doc = query("callers", &[input]);
    assert_eq!(doc["callers"], json!([]));
    assert_eq!(doc["stored"][0]["kind"], "materialized");
    assert_eq!(doc["stored"][0]["function"]["name"], "entry");
    assert_eq!(doc["stored"][0]["offset"], 0x1004);
}

#[test]
fn archive_call_relocations_keep_their_offset_and_argument_site() {
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
        write::{Object, Relocation, Symbol, SymbolSection},
    };
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    let bytes: Vec<_> = [0x00700513u32, 0x00000097, 0x000080e7, 0x00008067]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    obj.append_section_data(text, &bytes, 4);
    for (name, section, size) in [
        ("entry", SymbolSection::Section(text), 16),
        ("target", SymbolSection::Undefined, 0),
    ] {
        let symbol = obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: 0,
            size,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section,
            flags: SymbolFlags::None,
        });
        if name == "target" {
            obj.add_relocation(
                text,
                Relocation {
                    offset: 4,
                    symbol,
                    addend: 0,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_RISCV_CALL,
                    },
                },
            )
            .unwrap();
        }
    }
    let bytes = obj.write().unwrap();
    let archive = support::archive(&[(b"entry.o", &bytes)], false);
    let refs = sites(
        &query("callers", std::slice::from_ref(&archive)),
        "references",
    );
    assert_eq!(refs[0]["offset"], 4);
    assert_eq!(refs[0]["site"], 8);
    assert_eq!(refs[0]["kind"], "relocation");
    assert_eq!(refs[0]["relocation_kind"], "call");
    assert_eq!(refs[0]["target"], "target");
    let args = sites(&query("call-arguments", &[archive]), "sites");
    assert_eq!(args[0]["offset"], 8);
    assert_eq!(args[0]["arguments"][0]["value"], 7);
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
fn materialization_excludes_the_base_of_a_proven_transfer_and_keeps_auipc_stores() {
    // lui/addi t0,target; jalr ra,0(t0); ret
    let input = image(
        0x1000,
        &[0x000022b7, 0x23428293, 0x000280e7, 0x00008067],
        &[
            ("entry", 0x1000, 16, 2, 1),
            ("target", 0x2234, 0, 0, 0xfff1),
        ],
        false,
    );
    let doc = query("callers", &[input]);
    assert_eq!(sites(&doc, "references").len(), 1);
    assert_eq!(doc["stored"], json!([]), "{doc}");
    // auipc t0,1; addi t0,t0,0x234; sw t0,0(a0); ret
    let input = image(
        0x1000,
        &[0x00001297, 0x23428293, 0x00552023, 0x00008067],
        &[
            ("entry", 0x1000, 16, 2, 1),
            ("target", 0x2234, 0, 0, 0xfff1),
        ],
        false,
    );
    let doc = query("callers", &[input]);
    assert_eq!(doc["stored"][0]["kind"], "materialized");
    assert_eq!(doc["stored"][0]["offset"], 0x1004);
}

#[test]
fn unmatched_and_unresolved_transfers_do_not_match_a_target_name() {
    let inputs = [
        image(
            0x1000,
            &[0x000022b7, 0x234280e7, 0x000500e7, 0x00008067],
            &[("entry", 0x1000, 16, 2, 1)],
            false,
        ),
        image(0x3234, &[0x00008067], &[("target", 0x3234, 4, 2, 1)], false),
    ];
    assert_eq!(query("callers", &inputs)["callers"], json!([]));
    assert_eq!(query("call-arguments", &inputs)["callers"], json!([]));
}

#[test]
fn scanning_cli_never_emits_a_success_document_after_work_exhaustion() {
    let bytes = image(
        0x1000,
        &[0x00008067],
        &[("entry", 0x1000, 4, 2, 1), ("target", 0x2334, 0, 0, 0xfff1)],
        true,
    );
    let input = Executable::new(bytes.clone());
    let memory = WorkingMemory::new(1 << 20).unwrap();
    let mut meter = Work(u64::MAX);
    let targets = Targets::capture(
        std::slice::from_ref(&input),
        &["target".into()],
        &memory,
        &mut meter,
    )
    .unwrap();
    let mut scan = StoredScan::new(&memory, &mut meter).unwrap();
    scan.scan_data(std::slice::from_ref(&input), &targets, &memory, &mut meter)
        .unwrap();
    let budget = (u64::MAX - meter.0 - 1).to_string();
    let output = invoke("callers", &[bytes], &["--max-work-units", &budget]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "partial success must not be emitted"
    );
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "resource-limited");
}
