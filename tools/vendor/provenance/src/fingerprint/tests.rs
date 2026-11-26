use super::*;
use oer_tools_analysis_elf::rv32::{R_RISCV_BRANCH, R_RISCV_CALL, R_RISCV_HI20, R_RISCV_LO12_I};

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
/// (`cargo vendor-check provenance --chip <chip> --rebuild`).
#[test]
fn the_fingerprint_of_a_fixed_function_is_pinned() {
    let f = load(0x12345, 0x678, "sym_a");
    assert_eq!(
        f.code,
        "5b82f8e11fc46565952d28d6178f66f5c959b8b06be31e3dfdea5461b1b44a0a"
    );
}

#[test]
fn similarity_is_held_for_a_common_run_in_the_middle_at_both_ends_and_nowhere() {
    for (left, right, expected) in [
        // The middle: three of five on each side.
        (&[1, 2, 3, 4, 5][..], &[9, 2, 3, 4, 8][..], 600_000),
        // Both ends.
        (&[1, 2, 3, 4], &[1, 9, 9, 4], 500_000),
        // Nothing in common, and one side empty.
        (&[1, 2], &[3, 4], 0),
        (&[], &[1], 0),
        (&[1], &[], 0),
        // A repeated token is counted once a place.
        (&[1, 1, 1], &[1, 1], 800_000),
        // The order is part of it: one of three.
        (&[1, 2, 3], &[3, 2, 1], 333_333),
        // Different lengths; the shorter whole inside the longer.
        (&[1, 2, 3, 4, 5, 6, 7], &[2, 4, 6], 600_000),
        // The quotient is cut, not rounded: four million over seven.
        (&[1, 2, 3], &[1, 2, 4, 5], 571_428),
    ] {
        assert_eq!(similarity_ppm(left, right), expected, "{left:?} {right:?}");
        assert_eq!(similarity_ppm(right, left), expected, "{right:?} {left:?}");
    }
}

/// Every sequence of up to `length` tokens out of three.
fn sequences(length: usize) -> Vec<Vec<u64>> {
    let mut all = vec![vec![]];
    let mut last = vec![vec![]];
    for _ in 0..length {
        last = last
            .iter()
            .flat_map(|sequence: &Vec<u64>| {
                (0..3).map(move |token| {
                    let mut longer = sequence.clone();
                    longer.push(token);
                    longer
                })
            })
            .collect();
        all.extend(last.iter().cloned());
    }
    all
}

/// The longest common subsequence's length by its definition.
fn common(left: &[u64], right: &[u64]) -> u64 {
    match (left.split_first(), right.split_first()) {
        (Some((a, left_rest)), Some((b, right_rest))) if a == b => {
            1 + common(left_rest, right_rest)
        }
        (Some((_, left_rest)), Some((_, right_rest))) => {
            common(left_rest, right).max(common(left, right_rest))
        }
        _ => 0,
    }
}

#[test]
fn similarity_is_the_definition_s_for_every_short_pair() {
    let sequences = sequences(4);
    assert_eq!(sequences.len(), 121);
    for left in &sequences {
        for right in &sequences {
            let total = (left.len() + right.len()) as u64;
            // Two empty sequences have nothing to compare, and are alike.
            let expected = (2 * common(left, right) * 1_000_000)
                .checked_div(total)
                .unwrap_or(1_000_000);
            assert_eq!(
                u64::from(similarity_ppm(left, right)),
                expected,
                "{left:?} {right:?}"
            );
        }
    }
}

#[test]
fn a_body_is_cut_into_instructions_by_the_length_encoding() {
    // `li a5, 2` is two bytes; `lui a5, 0` is four.
    let short = [0x89, 0x47];
    let long = [0xb7, 0x07, 0x00, 0x00];
    let tokens = |bytes: &[u8]| function(bytes, vec![]).tokens;
    assert_eq!(tokens(&[]), [0_u64; 0]);
    assert_eq!(tokens(&short).len(), 1);
    assert_eq!(tokens(&long).len(), 1);
    assert_eq!(tokens(&[short.as_slice(), &long, &short].concat()).len(), 3);
    // An instruction's token is the same wherever it stands.
    assert_eq!(
        tokens(&[short.as_slice(), &long].concat()),
        [tokens(&short), tokens(&long)].concat()
    );
    // A body that ends inside an instruction: what is left is one token of
    // its own, which is not the whole instruction's.
    for cut in 1..4 {
        let partial = tokens(&[short.as_slice(), &long[..cut]].concat());
        assert_eq!(partial.len(), 2, "{cut}");
        assert_eq!(partial[0], tokens(&short)[0], "{cut}");
        assert_eq!(partial[1], tokens(&long[..cut])[0], "{cut}");
        assert_ne!(partial[1], tokens(&long)[0], "{cut}");
    }
    assert_eq!(tokens(&short[..1]).len(), 1);
}

#[test]
fn a_relocation_enters_the_token_of_the_instruction_that_holds_its_offset() {
    // `c.nop`, `nop`, `c.nop`: no bit a branch's relocation clears is set,
    // so a token changes only by the relocation that enters it.
    let bytes = [0x01, 0x00, 0x13, 0x00, 0x00, 0x00, 0x01, 0x00];
    let plain = function(&bytes, vec![]).tokens;
    let local = |offset: usize| {
        function(
            &bytes,
            vec![site(offset, R_RISCV_BRANCH, Reference::Local(4))],
        )
        .tokens
    };
    // Which tokens a relocation at each offset changes.
    for (offset, changed) in [
        (0, [true, false, false]),
        (2, [false, true, false]),
        // The second half of the four-byte instruction.
        (4, [false, true, false]),
        (6, [false, false, true]),
        // Past the body: in no instruction.
        (8, [false, false, false]),
        (usize::MAX, [false, false, false]),
    ] {
        let tokens = local(offset);
        assert_eq!(tokens.len(), 3, "{offset}");
        let differs: Vec<bool> = tokens.iter().zip(&plain).map(|(a, b)| a != b).collect();
        assert_eq!(differs, changed, "{offset}");
    }
    // Two relocations of one instruction enter it in the order of their
    // offsets, then of the list.
    let pair = |first: (usize, i64), second: (usize, i64)| {
        function(
            &bytes,
            vec![
                site(first.0, R_RISCV_BRANCH, Reference::Local(first.1)),
                site(second.0, R_RISCV_BRANCH, Reference::Local(second.1)),
            ],
        )
        .code
    };
    assert_eq!(pair((2, 4), (3, 8)), pair((3, 8), (2, 4)));
    assert_ne!(pair((2, 4), (2, 8)), pair((2, 8), (2, 4)));
}

/// FNV-1a over `bytes`, continued from `hash`.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A sequence of numbers that is the same on every run.
struct Numbers(u64);

impl Numbers {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

/// What `Function::of` gives for two thousand bodies of up to thirteen
/// bytes with up to four relocations each, at offsets inside and past the
/// body, held as one digest: the fingerprints, the tokens and the calls of
/// this module as it stands. It changes only with this algorithm or the
/// relocation table.
#[test]
fn the_fingerprints_of_a_space_of_bodies_are_held() {
    let mut numbers = Numbers(0x5eed);
    let mut digest = 0xcbf2_9ce4_8422_2325;
    for _ in 0..2000 {
        let bytes: Vec<u8> = (0..numbers.below(14))
            .map(|_| numbers.below(256).to_le_bytes()[0])
            .collect();
        let sites = (0..numbers.below(5))
            .map(|_| {
                let offset = usize::try_from(numbers.below(16)).unwrap();
                let r_type = [R_RISCV_BRANCH, R_RISCV_CALL, R_RISCV_HI20, R_RISCV_LO12_I]
                    [usize::try_from(numbers.below(4)).unwrap()];
                let reference = match numbers.below(4) {
                    0 => Reference::Local(i64::try_from(numbers.below(32)).unwrap() - 8),
                    1 => Reference::Symbol(format!("s{}", numbers.below(3))),
                    2 => Reference::Anonymous,
                    _ => Reference::Other,
                };
                site(offset, r_type, reference)
            })
            .collect();
        let function = function(&bytes, sites);
        digest = fnv(digest, format!("{bytes:02x?} {function:?}\n").as_bytes());
    }
    assert_eq!(digest, 0x49f9_5fdb_ac2e_beac, "{digest:#018x}");
}
