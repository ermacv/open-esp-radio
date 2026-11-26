//! What the decoder writes, held as it stands: one encoding with its text
//! for every mnemonic, rounding mode and register list of the forms this
//! crate decodes itself, every register's name in every place, and a digest
//! of the text and the structure of whole encoding spaces.
//!
//! The texts are the assembler's (the encodings of the other tests come from
//! LLVM) and the digests are this decoder's own output: a change to either
//! is a change of what the decoder gives its consumers.

use crate::{Extensions, decode};

/// The width and text of the instruction at the start of `bytes`.
fn text(bytes: &[u8]) -> Option<(usize, String)> {
    decode(bytes, Extensions::ALL).map(|(instruction, width)| (width, instruction.to_string()))
}

/// A 32-bit word of the R format from its fields.
const fn word(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

/// The Zba, Zbb, Zbs and Zicsr forms and the single-precision forms.
const WORDS: &[(u32, &str)] = &[
    (0x20c5a533, "sh1add a0, a1, a2"),
    (0x20c5c533, "sh2add a0, a1, a2"),
    (0x20f4e2b3, "sh3add t0, s1, a5"),
    (0x40c5f533, "andn a0, a1, a2"),
    (0x40c5e533, "orn a0, a1, a2"),
    (0x40c5c533, "xnor a0, a1, a2"),
    (0x0ac5c533, "min a0, a1, a2"),
    (0x0ac5d533, "minu a0, a1, a2"),
    (0x0ac5e533, "max a0, a1, a2"),
    (0x0ac5f533, "maxu a0, a1, a2"),
    (0x60c59533, "rol a0, a1, a2"),
    (0x60c5d533, "ror a0, a1, a2"),
    (0x6075d513, "rori a0, a1, 7"),
    (0x60059513, "clz a0, a1"),
    (0x60159513, "ctz a0, a1"),
    (0x60259513, "cpop a0, a1"),
    (0x60459513, "sext.b a0, a1"),
    (0x60559513, "sext.h a0, a1"),
    (0x0805c533, "zext.h a0, a1"),
    (0x2875d513, "orc.b a0, a1"),
    (0x6985d513, "rev8 a0, a1"),
    (0x28c59533, "bset a0, a1, a2"),
    (0x29f59513, "bseti a0, a1, 31"),
    (0x48c59533, "bclr a0, a1, a2"),
    (0x48359513, "bclri a0, a1, 3"),
    (0x68c59533, "binv a0, a1, a2"),
    (0x68459513, "binvi a0, a1, 4"),
    (0x48c5d533, "bext a0, a1, a2"),
    (0x4855d513, "bexti a0, a1, 5"),
    (0xf14026f3, "csrrs a3, 0xf14, zero"),
    (0x34011073, "csrrw zero, 0x340, sp"),
    (0x30047573, "csrrci a0, 0x300, 8"),
    (0x7e15b2f3, "csrrc t0, 0x7e1, a1"),
    (0x30045573, "csrrwi a0, 0x300, 8"),
    (0x30046573, "csrrsi a0, 0x300, 8"),
    (0x68c5a547, "fmsub.s fa0, fa1, fa2, fa3, rdn"),
    (0x68c5b54b, "fnmsub.s fa0, fa1, fa2, fa3, rup"),
    (0x08c5c553, "fsub.s fa0, fa1, fa2, rmm"),
    (0x10c5f553, "fmul.s fa0, fa1, fa2"),
    (0x18c58553, "fdiv.s fa0, fa1, fa2, rne"),
    (0x20c58553, "fsgnj.s fa0, fa1, fa2"),
    (0x20c5a553, "fsgnjx.s fa0, fa1, fa2"),
    (0x28c58553, "fmin.s fa0, fa1, fa2"),
    (0xc0159553, "fcvt.wu.s a0, fa1, rtz"),
    (0xd005f553, "fcvt.s.w fa0, a1"),
    (0xffc12507, "flw fa0, -4(sp)"),
    (0x7e97ae27, "fsw fs1, 2044(a5)"),
    (0x68c59543, "fmadd.s fa0, fa1, fa2, fa3, rtz"),
    (0x68c5f54f, "fnmadd.s fa0, fa1, fa2, fa3"),
    (0x0020f053, "fadd.s ft0, ft1, ft2"),
    (0x58058553, "fsqrt.s fa0, fa1, rne"),
    (0xc0051553, "fcvt.w.s a0, fa0, rtz"),
    (0xd015f553, "fcvt.s.wu fa0, a1"),
    (0xa0b52553, "feq.s a0, fa0, fa1"),
    (0xa01012d3, "flt.s t0, ft0, ft1"),
    (0xa0c585d3, "fle.s a1, fa1, fa2"),
    (0xe0050553, "fmv.x.w a0, fa0"),
    (0xf0000553, "fmv.w.x fa0, zero"),
    (0xe0061653, "fclass.s a2, fa2"),
    (0x20c59553, "fsgnjn.s fa0, fa1, fa2"),
    (0x28c59553, "fmax.s fa0, fa1, fa2"),
];

/// The Zcb forms, every Zcmp form with every register list at its least and
/// greatest adjustment, every pair of the Zcmp moves' saved registers, and
/// the compressed single-precision loads and stores.
const HALVES: &[(u16, &str)] = &[
    (0x9c61, "zext.b s0, s0"),
    (0x9ce5, "sext.b s1, s1"),
    (0x9d69, "zext.h a0, a0"),
    (0x9ded, "sext.h a1, a1"),
    (0x9e75, "not a2, a2"),
    (0x9ed9, "mul a3, a3, a4"),
    (0x81e8, "c.lbu a0, 3(a1)"),
    (0x8430, "c.lhu a2, 2(s0)"),
    (0x87d4, "c.lh a3, 0(a5)"),
    (0x88d8, "c.sb a4, 1(s1)"),
    (0x8d3c, "c.sh a5, 2(a0)"),
    (0xb842, "cm.push {ra}, -16"),
    (0xb84e, "cm.push {ra}, -64"),
    (0xb852, "cm.push {ra, s0}, -16"),
    (0xb85e, "cm.push {ra, s0}, -64"),
    (0xb862, "cm.push {ra, s0-s1}, -16"),
    (0xb86e, "cm.push {ra, s0-s1}, -64"),
    (0xb872, "cm.push {ra, s0-s2}, -16"),
    (0xb87e, "cm.push {ra, s0-s2}, -64"),
    (0xb882, "cm.push {ra, s0-s3}, -32"),
    (0xb88e, "cm.push {ra, s0-s3}, -80"),
    (0xb892, "cm.push {ra, s0-s4}, -32"),
    (0xb89e, "cm.push {ra, s0-s4}, -80"),
    (0xb8a2, "cm.push {ra, s0-s5}, -32"),
    (0xb8ae, "cm.push {ra, s0-s5}, -80"),
    (0xb8b2, "cm.push {ra, s0-s6}, -32"),
    (0xb8be, "cm.push {ra, s0-s6}, -80"),
    (0xb8c2, "cm.push {ra, s0-s7}, -48"),
    (0xb8ce, "cm.push {ra, s0-s7}, -96"),
    (0xb8d2, "cm.push {ra, s0-s8}, -48"),
    (0xb8de, "cm.push {ra, s0-s8}, -96"),
    (0xb8e2, "cm.push {ra, s0-s9}, -48"),
    (0xb8ee, "cm.push {ra, s0-s9}, -96"),
    (0xb8f2, "cm.push {ra, s0-s11}, -64"),
    (0xb8fe, "cm.push {ra, s0-s11}, -112"),
    (0xba42, "cm.pop {ra}, 16"),
    (0xba4e, "cm.pop {ra}, 64"),
    (0xba52, "cm.pop {ra, s0}, 16"),
    (0xba5e, "cm.pop {ra, s0}, 64"),
    (0xba62, "cm.pop {ra, s0-s1}, 16"),
    (0xba6e, "cm.pop {ra, s0-s1}, 64"),
    (0xba72, "cm.pop {ra, s0-s2}, 16"),
    (0xba7e, "cm.pop {ra, s0-s2}, 64"),
    (0xba82, "cm.pop {ra, s0-s3}, 32"),
    (0xba8e, "cm.pop {ra, s0-s3}, 80"),
    (0xba92, "cm.pop {ra, s0-s4}, 32"),
    (0xba9e, "cm.pop {ra, s0-s4}, 80"),
    (0xbaa2, "cm.pop {ra, s0-s5}, 32"),
    (0xbaae, "cm.pop {ra, s0-s5}, 80"),
    (0xbab2, "cm.pop {ra, s0-s6}, 32"),
    (0xbabe, "cm.pop {ra, s0-s6}, 80"),
    (0xbac2, "cm.pop {ra, s0-s7}, 48"),
    (0xbace, "cm.pop {ra, s0-s7}, 96"),
    (0xbad2, "cm.pop {ra, s0-s8}, 48"),
    (0xbade, "cm.pop {ra, s0-s8}, 96"),
    (0xbae2, "cm.pop {ra, s0-s9}, 48"),
    (0xbaee, "cm.pop {ra, s0-s9}, 96"),
    (0xbaf2, "cm.pop {ra, s0-s11}, 64"),
    (0xbafe, "cm.pop {ra, s0-s11}, 112"),
    (0xbe42, "cm.popret {ra}, 16"),
    (0xbe4e, "cm.popret {ra}, 64"),
    (0xbe52, "cm.popret {ra, s0}, 16"),
    (0xbe5e, "cm.popret {ra, s0}, 64"),
    (0xbe62, "cm.popret {ra, s0-s1}, 16"),
    (0xbe6e, "cm.popret {ra, s0-s1}, 64"),
    (0xbe72, "cm.popret {ra, s0-s2}, 16"),
    (0xbe7e, "cm.popret {ra, s0-s2}, 64"),
    (0xbe82, "cm.popret {ra, s0-s3}, 32"),
    (0xbe8e, "cm.popret {ra, s0-s3}, 80"),
    (0xbe92, "cm.popret {ra, s0-s4}, 32"),
    (0xbe9e, "cm.popret {ra, s0-s4}, 80"),
    (0xbea2, "cm.popret {ra, s0-s5}, 32"),
    (0xbeae, "cm.popret {ra, s0-s5}, 80"),
    (0xbeb2, "cm.popret {ra, s0-s6}, 32"),
    (0xbebe, "cm.popret {ra, s0-s6}, 80"),
    (0xbec2, "cm.popret {ra, s0-s7}, 48"),
    (0xbece, "cm.popret {ra, s0-s7}, 96"),
    (0xbed2, "cm.popret {ra, s0-s8}, 48"),
    (0xbede, "cm.popret {ra, s0-s8}, 96"),
    (0xbee2, "cm.popret {ra, s0-s9}, 48"),
    (0xbeee, "cm.popret {ra, s0-s9}, 96"),
    (0xbef2, "cm.popret {ra, s0-s11}, 64"),
    (0xbefe, "cm.popret {ra, s0-s11}, 112"),
    (0xbc42, "cm.popretz {ra}, 16"),
    (0xbc4e, "cm.popretz {ra}, 64"),
    (0xbc52, "cm.popretz {ra, s0}, 16"),
    (0xbc5e, "cm.popretz {ra, s0}, 64"),
    (0xbc62, "cm.popretz {ra, s0-s1}, 16"),
    (0xbc6e, "cm.popretz {ra, s0-s1}, 64"),
    (0xbc72, "cm.popretz {ra, s0-s2}, 16"),
    (0xbc7e, "cm.popretz {ra, s0-s2}, 64"),
    (0xbc82, "cm.popretz {ra, s0-s3}, 32"),
    (0xbc8e, "cm.popretz {ra, s0-s3}, 80"),
    (0xbc92, "cm.popretz {ra, s0-s4}, 32"),
    (0xbc9e, "cm.popretz {ra, s0-s4}, 80"),
    (0xbca2, "cm.popretz {ra, s0-s5}, 32"),
    (0xbcae, "cm.popretz {ra, s0-s5}, 80"),
    (0xbcb2, "cm.popretz {ra, s0-s6}, 32"),
    (0xbcbe, "cm.popretz {ra, s0-s6}, 80"),
    (0xbcc2, "cm.popretz {ra, s0-s7}, 48"),
    (0xbcce, "cm.popretz {ra, s0-s7}, 96"),
    (0xbcd2, "cm.popretz {ra, s0-s8}, 48"),
    (0xbcde, "cm.popretz {ra, s0-s8}, 96"),
    (0xbce2, "cm.popretz {ra, s0-s9}, 48"),
    (0xbcee, "cm.popretz {ra, s0-s9}, 96"),
    (0xbcf2, "cm.popretz {ra, s0-s11}, 64"),
    (0xbcfe, "cm.popretz {ra, s0-s11}, 112"),
    (0xac3e, "cm.mvsa01 s0, s7"),
    (0xac7e, "cm.mva01s s0, s7"),
    (0xacba, "cm.mvsa01 s1, s6"),
    (0xacfa, "cm.mva01s s1, s6"),
    (0xad36, "cm.mvsa01 s2, s5"),
    (0xad76, "cm.mva01s s2, s5"),
    (0xadb2, "cm.mvsa01 s3, s4"),
    (0xadf2, "cm.mva01s s3, s4"),
    (0xae2e, "cm.mvsa01 s4, s3"),
    (0xae6e, "cm.mva01s s4, s3"),
    (0xaeaa, "cm.mvsa01 s5, s2"),
    (0xaeea, "cm.mva01s s5, s2"),
    (0xaf26, "cm.mvsa01 s6, s1"),
    (0xaf66, "cm.mva01s s6, s1"),
    (0xafa2, "cm.mvsa01 s7, s0"),
    (0xafe2, "cm.mva01s s7, s0"),
    (0x70fe, "flw ft1, 252(sp)"),
    (0xe0be, "fsw fa5, 64(sp)"),
    (0x7f74, "flw fa3, 124(a4)"),
    (0xe0c0, "fsw fs0, 4(s1)"),
];

#[test]
fn every_mnemonic_has_one_encoding_with_its_text() {
    for (encoding, expected) in WORDS {
        assert_eq!(
            text(&encoding.to_le_bytes()),
            Some((4, (*expected).to_owned())),
            "{encoding:#010x}"
        );
    }
    for (encoding, expected) in HALVES {
        assert_eq!(
            text(&encoding.to_le_bytes()),
            Some((2, (*expected).to_owned())),
            "{encoding:#06x}"
        );
    }
}

const INTEGER: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];
const FLOAT: [&str; 32] = [
    "ft0", "ft1", "ft2", "ft3", "ft4", "ft5", "ft6", "ft7", "fs0", "fs1", "fa0", "fa1", "fa2",
    "fa3", "fa4", "fa5", "fa6", "fa7", "fs2", "fs3", "fs4", "fs5", "fs6", "fs7", "fs8", "fs9",
    "fs10", "fs11", "ft8", "ft9", "ft10", "ft11",
];

#[test]
fn every_register_has_its_name_in_every_place() {
    let written = |encoding: u32| text(&encoding.to_le_bytes()).map(|(_, text)| text);
    for (x, name) in (0..).zip(INTEGER) {
        // An integer form's destination and both operands.
        for (encoding, expected) in [
            (
                word(0x10, 12, 11, 2, x, 0x33),
                format!("sh1add {name}, a1, a2"),
            ),
            (
                word(0x10, 12, x, 2, 10, 0x33),
                format!("sh1add a0, {name}, a2"),
            ),
            (
                word(0x10, x, 11, 2, 10, 0x33),
                format!("sh1add a0, a1, {name}"),
            ),
            // A CSR access's destination and source.
            (
                word(0x1a, 0, 2, 1, x, 0x73),
                format!("csrrw {name}, 0x340, sp"),
            ),
            (
                word(0x1a, 0, x, 2, 10, 0x73),
                format!("csrrs a0, 0x340, {name}"),
            ),
            // A single-precision form's integer destination, source and base.
            (
                word(0x70, 0, 10, 0, x, 0x53),
                format!("fmv.x.w {name}, fa0"),
            ),
            (
                word(0x78, 0, x, 0, 10, 0x53),
                format!("fmv.w.x fa0, {name}"),
            ),
            (word(0, 0, x, 2, 10, 0x07), format!("flw fa0, 0({name})")),
            (word(0, 10, x, 2, 0, 0x27), format!("fsw fa0, 0({name})")),
        ] {
            assert_eq!(written(encoding), Some(expected), "{encoding:#010x}");
        }
    }
    for (f, name) in (0..).zip(FLOAT) {
        for (encoding, expected) in [
            (
                word(0, 12, 11, 7, f, 0x53),
                format!("fadd.s {name}, fa1, fa2"),
            ),
            (
                word(0, 12, f, 7, 10, 0x53),
                format!("fadd.s fa0, {name}, fa2"),
            ),
            (
                word(0, f, 11, 7, 10, 0x53),
                format!("fadd.s fa0, fa1, {name}"),
            ),
            (
                (f << 27) | word(0, 12, 11, 7, 10, 0x43),
                format!("fmadd.s fa0, fa1, fa2, {name}"),
            ),
            (word(0, 0, 11, 2, f, 0x07), format!("flw {name}, 0(a1)")),
            (word(0, f, 11, 2, 0, 0x27), format!("fsw {name}, 0(a1)")),
        ] {
            assert_eq!(written(encoding), Some(expected), "{encoding:#010x}");
        }
    }
    // The compressed forms' three-bit fields name x8..x15 and f8..f15, and
    // the stack-pointer forms every register of the single-precision file.
    for (field, (x, f)) in (0u16..).zip(INTEGER.into_iter().zip(FLOAT).skip(8).take(8)) {
        for (encoding, expected) in [
            (
                0x8000 | (field << 7) | (field << 2),
                format!("c.lbu {x}, 0({x})"),
            ),
            (
                0x6000 | (field << 7) | (field << 2),
                format!("flw {f}, 0({x})"),
            ),
            (
                0xe000 | (field << 7) | (field << 2),
                format!("fsw {f}, 0({x})"),
            ),
            (0x9c61 | (field << 7), format!("zext.b {x}, {x}")),
        ] {
            assert_eq!(
                text(&encoding.to_le_bytes()),
                Some((2, expected)),
                "{encoding:#06x}"
            );
        }
    }
    for (field, name) in (0u16..).zip(FLOAT) {
        for (encoding, expected) in [
            (0x6002 | (field << 7), format!("flw {name}, 0(sp)")),
            (0xe002 | (field << 2), format!("fsw {name}, 0(sp)")),
        ] {
            assert_eq!(
                text(&encoding.to_le_bytes()),
                Some((2, expected)),
                "{encoding:#06x}"
            );
        }
    }
}

/// FNV-1a over `bytes`, continued from `hash`.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// `hash` continued with what `bytes` decode to: the width, the text and the
/// typed instruction, or that they do not decode.
fn decoded(hash: u64, bytes: &[u8]) -> u64 {
    let line = match decode(bytes, Extensions::ALL) {
        Some((instruction, width)) => {
            format!("{bytes:02x?} {width} {instruction} {instruction:?}\n")
        }
        None => format!("{bytes:02x?} none\n"),
    };
    fnv(hash, line.as_bytes())
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// The major opcodes of the 32-bit forms this crate decodes itself.
const MAJORS: [u32; 10] = [0x33, 0x13, 0x73, 0x07, 0x27, 0x43, 0x47, 0x4b, 0x4f, 0x53];

/// Every 16-bit encoding; then every slice too short for its instruction.
fn halves_digest() -> (u64, u64) {
    let whole = (0..=u16::MAX).fold(FNV_OFFSET, |hash, half| decoded(hash, &half.to_le_bytes()));
    let mut short = decoded(FNV_OFFSET, &[]);
    for byte in 0..=u8::MAX {
        short = decoded(short, &[byte]);
        // A 32-bit encoding lent three bytes, with every value of its first.
        short = decoded(short, &[byte, 0xc5, 0x20]);
    }
    (whole, short)
}

/// Every `funct7`, `funct3` and `rs2` of `major` with four pairs of the other
/// registers; then every register number in every place of every `funct7`
/// and `funct3`.
fn major_digest(major: u32) -> u64 {
    let mut hash = FNV_OFFSET;
    for funct7 in 0..128 {
        for funct3 in 0..8 {
            for rs2 in 0..32 {
                for (rd, rs1) in [(0, 0), (8, 9), (10, 11), (31, 17)] {
                    hash = decoded(
                        hash,
                        &word(funct7, rs2, rs1, funct3, rd, major).to_le_bytes(),
                    );
                }
            }
            for register in 0..32 {
                let rs2 = (register * 7 + 3) % 32;
                let encoding = word(funct7, rs2, 31 - register, funct3, register, major);
                hash = decoded(hash, &encoding.to_le_bytes());
            }
        }
    }
    hash
}

#[test]
fn the_encoding_spaces_decode_as_they_did() {
    let (whole, short) = halves_digest();
    let majors = MAJORS.map(major_digest);
    assert_eq!(
        (whole, short, majors),
        (HALVES_DIGEST, SHORT_DIGEST, MAJOR_DIGESTS),
        "the decoded text or structure of an encoding changed"
    );
}

const HALVES_DIGEST: u64 = 0x4ccc_e381_c310_d2e3;
const SHORT_DIGEST: u64 = 0xe59b_069a_c1ed_b1b7;
const MAJOR_DIGESTS: [u64; 10] = [
    0x6346_4dcd_a3b4_24c3,
    0x8dbb_108b_9592_04b4,
    0x8e98_19b1_4ae0_07fa,
    0x5a3e_0a89_2f21_0263,
    0x7e0b_ba92_2f28_4871,
    0x2985_f6de_6e9b_df15,
    0x81fc_ce88_d757_62dd,
    0x86cb_5734_ec1b_85cd,
    0x286a_a8e0_db28_13d5,
    0xea9d_9938_ba72_ebd4,
];
