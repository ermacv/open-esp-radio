use super::*;
use oer_elf::rv32::{R_RISCV_BRANCH, R_RISCV_CALL, R_RISCV_HI20, R_RISCV_LO12_I};

fn site(offset: usize, r_type: u32, reference: Reference) -> Site {
    Site {
        offset,
        r_type,
        reference,
    }
}

fn function(bytes: &[u8], sites: Vec<Site>) -> Function {
    Function::of(&Code {
        member: "a.o".into(),
        name: "f",
        bytes,
        sites,
    })
}

/// `lui a5, IMM; lw a4, IMM(a5)` with both immediates relocated.
fn load(high: u32, low: u32, name: &str) -> Function {
    let lui = 0x0000_07b7 | (high << 12);
    let lw = 0x0007_a703 | (low << 20);
    let code = [lui.to_le_bytes(), lw.to_le_bytes()].concat();
    function(
        &code,
        vec![
            site(0, R_RISCV_HI20, Reference::Symbol(name.into())),
            site(4, R_RISCV_LO12_I, Reference::Symbol(name.into())),
        ],
    )
}

#[test]
fn relocated_fields_do_not_enter_the_fingerprint() {
    let a = load(0x12345, 0x678, "sym_a");
    let b = load(0x54321, 0x123, "sym_a");
    assert_eq!(a.code, b.code);
    assert_eq!(a.named, b.named);
}

#[test]
fn a_renamed_target_changes_only_the_named_fingerprint() {
    let a = load(0, 0, "sym_a");
    let b = load(0, 0, "sym_b");
    assert_eq!(a.code, b.code);
    assert_ne!(a.named, b.named);
}

#[test]
fn unrelocated_code_changes_the_code_fingerprint() {
    // `li a5, 2` against `li a5, 6`.
    let a = function(&[0x89, 0x47], vec![]);
    let b = function(&[0x99, 0x47], vec![]);
    assert_ne!(a.code, b.code);
    assert_eq!(a.tokens.len(), 1);
}

#[test]
fn call_immediates_do_not_change_identity_but_registers_do() {
    // auipc ra, 0x12 ; jalr ra, 0x34(ra)
    let first = [0x97, 0x20, 0x01, 0x00, 0xe7, 0x80, 0x40, 0x03];
    // auipc ra, 0x56 ; jalr ra, 0x78(ra)
    let second = [0x97, 0x60, 0x05, 0x00, 0xe7, 0x80, 0x80, 0x07];
    // auipc t1 instead of ra
    let third = [0x17, 0x63, 0x05, 0x00, 0xe7, 0x80, 0x80, 0x07];
    let call = |name: &str| vec![site(0, R_RISCV_CALL, Reference::Symbol(name.into()))];
    let a = function(&first, call("r_sym_bt_first"));
    let b = function(&second, call("r_named_callee"));
    let c = function(&third, call("r_named_callee"));
    assert_eq!(a.code, b.code);
    assert_ne!(a.code, c.code);
    assert_eq!(b.calls, ["r_named_callee"]);
}

#[test]
fn local_label_offsets_are_part_of_identity() {
    let bytes = [0x63, 0x04, 0x00, 0x00];
    let a = function(&bytes, vec![site(0, R_RISCV_BRANCH, Reference::Local(8))]);
    let b = function(&bytes, vec![site(0, R_RISCV_BRANCH, Reference::Local(12))]);
    assert_ne!(a.code, b.code);
}

#[test]
fn similarity_counts_the_common_subsequence() {
    assert_eq!(similarity_ppm(&[1, 2, 3, 4], &[1, 2, 3, 4]), 1_000_000);
    assert_eq!(similarity_ppm(&[1, 2, 3, 4], &[1, 9, 3, 4]), 750_000);
    assert_eq!(similarity_ppm(&[1, 2], &[3, 4]), 0);
    assert_eq!(similarity_ppm(&[], &[]), 1_000_000);
}

/// The fingerprint of a fixed function, pinned: it changes only with the
/// relocation table or this algorithm, and then every registered vendor
/// fingerprint must be regenerated with it
/// (`cargo verification provenance --chip <chip> --rebuild`).
#[test]
fn the_fingerprint_of_a_fixed_function_is_pinned() {
    let f = load(0x12345, 0x678, "sym_a");
    assert_eq!(
        f.code,
        "5b82f8e11fc46565952d28d6178f66f5c959b8b06be31e3dfdea5461b1b44a0a"
    );
}
