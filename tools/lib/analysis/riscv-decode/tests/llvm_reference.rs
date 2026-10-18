//! Which encodings decode, against the pinned toolchain's `llvm-objdump`.
//!
//! Every 16-bit encoding, and for every 32-bit major opcode every `funct3`
//! and every value of bits 31:20 with sampled `rd` and `rs1` fields, must
//! decode with `Extensions::ALL` exactly when LLVM disassembles it with the
//! same extensions, except for the differences `expected` names. Field values
//! are covered by the unit tests; this checks the accepted encoding space.
//!
//! Ignored by default because it runs an external tool:
//! `cargo test -p oer-riscv-decode --test llvm_reference -- --ignored`. It uses
//! `$LLVM_OBJDUMP`, or the `llvm-objdump` of the `llvm-tools` component that
//! `rust-toolchain.toml` installs.

use oer_riscv_decode::{Extensions, decode};
use std::{env, fs, path::PathBuf, process::Command};

const ATTRIBUTES: &str = "+m,+a,+f,+c,+zba,+zbb,+zbs,+zcb,+zcmp";

fn objdump() -> PathBuf {
    oer_toolchain::program(oer_toolchain::Tool::LlvmObjdump)
        .unwrap()
        .into()
}

/// A relocatable RV32 ELF whose only content is `code` in `.text`.
fn elf(code: &[u8]) -> Vec<u8> {
    let names = b"\0.text\0.shstrtab\0";
    let text = 52;
    let strings = text + code.len();
    let sections = (strings + names.len()).next_multiple_of(4);
    let mut out = Vec::with_capacity(sections + 120);
    out.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    for half in [1u16, 243] {
        out.extend_from_slice(&half.to_le_bytes()); // ET_REL, EM_RISCV
    }
    for word in [1u32, 0, 0, sections as u32, 1] {
        out.extend_from_slice(&word.to_le_bytes()); // version .. e_flags (RVC)
    }
    for half in [52u16, 0, 0, 40, 3, 2] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(code);
    out.extend_from_slice(names);
    out.resize(sections, 0);
    let header = |out: &mut Vec<u8>, fields: [u32; 10]| {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    };
    header(&mut out, [0; 10]);
    // .text: PROGBITS, ALLOC | EXECINSTR.
    header(
        &mut out,
        [1, 1, 6, 0, text as u32, code.len() as u32, 0, 0, 2, 0],
    );
    // .shstrtab: STRTAB.
    header(
        &mut out,
        [7, 3, 0, 0, strings as u32, names.len() as u32, 0, 0, 1, 0],
    );
    out
}

/// LLVM's text for each probe, by probe index: every probe occupies `slot`
/// bytes.
fn disassemble(code: &[u8], slot: usize) -> Vec<Option<String>> {
    let path = env::temp_dir().join(format!("oer-riscv-decode-{}.o", std::process::id()));
    fs::write(&path, elf(code)).unwrap();
    let output = Command::new(objdump())
        .args(["-d", "-M", "no-aliases", &format!("--mattr={ATTRIBUTES}")])
        .arg(&path)
        .output()
        .expect("llvm-objdump runs");
    fs::remove_file(&path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut texts = vec![None; code.len() / slot];
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let Some((address, rest)) = line.trim_start().split_once(':') else {
            continue;
        };
        let Ok(address) = usize::from_str_radix(address, 16) else {
            continue;
        };
        if address % slot != 0 {
            continue;
        }
        // Encoding bytes, then the instruction after a tab.
        let text = rest.split_once('\t').map_or("", |(_, text)| text).trim();
        texts[address / slot] = Some(text.to_owned());
    }
    texts
}

fn llvm_decodes(text: &Option<String>) -> bool {
    text.as_deref().is_some_and(|text| {
        !text.is_empty() && !text.starts_with("<unknown>") && !text.starts_with("c.unimp")
    })
}

/// Why the decoder and LLVM may disagree about `encoding`, given whether the
/// decoder accepted it.
fn expected(encoding: u32, compressed: bool, decoded: bool) -> Option<&'static str> {
    if compressed {
        let half = encoding as u16;
        let mvsa01 = half & 0xfc63 == 0xac22 && (half >> 7) & 7 == (half >> 2) & 7;
        return (!decoded && mvsa01)
            .then_some("cm.mvsa01 reserves equal registers (Zcmp); LLVM decodes it");
    }
    let (major, funct3) = (encoding & 0x7f, (encoding >> 12) & 7);
    match (major, funct3, decoded) {
        (0x73, _, false) => Some("Zicsr and privileged instructions are outside Extensions::ALL"),
        (0x0f, 1, false) => Some("Zifencei is outside Extensions::ALL"),
        (0x0f, 0, true) => Some(
            "FENCE with nonzero rd, rs1 or a reserved fm: base implementations ignore \
             those fields; LLVM does not decode them",
        ),
        _ => None,
    }
}

fn compare(encodings: &[u32], compressed: bool, unexpected: &mut Vec<String>) {
    let slot = 4;
    let mut code = Vec::with_capacity(encodings.len() * slot);
    for &encoding in encodings {
        if compressed {
            // The probe, then c.nop, so a 32-bit reading cannot swallow the next probe.
            code.extend_from_slice(&(encoding as u16).to_le_bytes());
            code.extend_from_slice(&1u16.to_le_bytes());
        } else {
            code.extend_from_slice(&encoding.to_le_bytes());
        }
    }
    let texts = disassemble(&code, slot);
    for (&encoding, text) in encodings.iter().zip(&texts) {
        let probe = if compressed {
            (encoding as u16).to_le_bytes().to_vec()
        } else {
            encoding.to_le_bytes().to_vec()
        };
        let ours = decode(&probe, Extensions::ALL);
        let decoded = ours.is_some_and(|(_, width)| width == probe.len());
        if decoded != llvm_decodes(text) && expected(encoding, compressed, decoded).is_none() {
            unexpected.push(format!(
                "{encoding:0width$x}: decoder {} LLVM {:?}",
                if decoded { "accepts" } else { "rejects" },
                text,
                width = probe.len() * 2
            ));
        }
    }
}

#[test]
#[ignore = "runs the toolchain's llvm-objdump"]
fn the_accepted_encodings_are_llvm_s() {
    let mut unexpected = Vec::new();
    let halves: Vec<u32> = (0..=u16::MAX)
        .filter(|h| h & 3 != 3)
        .map(u32::from)
        .collect();
    compare(&halves, true, &mut unexpected);
    for major in (0..128u32).filter(|m| m & 3 == 3 && m & 0x1c != 0x1c) {
        let words: Vec<u32> = (0..8)
            .flat_map(|funct3| (0..4096).map(move |high| (high << 20) | (funct3 << 12) | major))
            .flat_map(|word| {
                [(0, 0), (1, 10), (10, 31), (31, 1)].map(|(rd, rs1)| word | (rd << 7) | (rs1 << 15))
            })
            .collect();
        compare(&words, false, &mut unexpected);
    }
    assert!(
        unexpected.is_empty(),
        "{} unexpected differences, first: {:#?}",
        unexpected.len(),
        &unexpected[..unexpected.len().min(20)]
    );
}
