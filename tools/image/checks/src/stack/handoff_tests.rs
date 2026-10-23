use super::*;
use crate::stack::handoff::*;

const AUIPC_SP_TO_RODATA: u32 = 0x0000_1117; // auipc sp, 0x1000
const AUIPC_SP: u32 = 0x0000_0117; // auipc sp, 0
const ADDI_SP_0: u32 = 0x0001_0113; // addi sp, sp, 0
const AUIPC_RA: u32 = 0x0000_0097; // auipc ra, 0
const JALR_RA_16: u32 = 0x0100_80e7; // jalr ra, 16(ra)
const NOP: u32 = 0x0000_0013;
const LUI_T1_0X5000: u32 = 0x0000_5337; // lui t1, 0x5000
const LUI_T1_0X50014000: u32 = 0x5001_4337; // lui t1, 0x50014000
const ADDI_T1_MINUS_0X300: u32 = 0xd003_0313; // addi t1, t1, -0x300
const ADDI_T1_0: u32 = 0x0003_0313; // addi t1, t1, 0
const JR_T1: u32 = 0x0003_0067; // jalr zero, 0(t1)
const BEQ_TO_PLUS_8: u32 = 0x0000_0463; // beq zero, zero, +8
const ADDI_SP_MINUS_4: u32 = 0xffc1_0113; // addi sp, sp, -4
const SW_A0_0_SP: u32 = 0x00a1_2023; // sw a0, 0(sp)

fn code(name: &'static str, words: Vec<u32>) -> Code {
    Code {
        name,
        words,
        frame: Some(0),
    }
}

fn scan(code: Vec<Code>, entry: &str, top: u32) -> std::result::Result<(), String> {
    let bytes = executable(&code, &[]);
    let elf = oer_analysis_elf::Elf::executable(&bytes).unwrap();
    let address = elf.address(entry).unwrap() as u32;
    installs_stack_pointer(&elf, address, top, 0, &mut Default::default())
}

#[test]
fn an_entry_that_installs_the_stack_pointer_before_anything_reads_it_passes() {
    scan(
        vec![code(
            "entry",
            vec![AUIPC_SP_TO_RODATA, ADDI_SP_0, SW_A0_0_SP, RET],
        )],
        "entry",
        RODATA,
    )
    .unwrap();
    // Through a call whose callee installs it.
    scan(
        vec![
            code("entry", vec![AUIPC_RA, JALR_RA_16, NOP, RET]),
            code("callee", vec![AUIPC_SP, ADDI_SP_0, RET]),
        ],
        "entry",
        TEXT + 16,
    )
    .unwrap();
}

#[test]
fn an_entry_that_uses_the_stack_first_or_installs_another_stack_is_refused() {
    for (words, expected) in [
        (
            vec![SP_DOWN_16, AUIPC_SP_TO_RODATA, ADDI_SP_0, RET],
            "sp is ",
        ),
        (
            vec![SW_A0_0_SP, AUIPC_SP_TO_RODATA, ADDI_SP_0, RET],
            "sp is ",
        ),
        (
            vec![AUIPC_SP, ADDI_SP_0, RET],
            "not set to the CPU0 task stack's top",
        ),
        (
            vec![AUIPC_SP_TO_RODATA, NOP, RET],
            "not set to the CPU0 task stack's top",
        ),
        // A path that returns before installing the stack pointer.
        (
            vec![BEQ_TO_PLUS_8, RET, AUIPC_SP_TO_RODATA, ADDI_SP_0, RET],
            "not a call",
        ),
    ] {
        let error = scan(vec![code("entry", words)], "entry", RODATA).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
    // A callee that reads the stack pointer before installing it.
    let error = scan(
        vec![
            code("entry", vec![AUIPC_RA, JALR_RA_16, NOP, RET]),
            code("callee", vec![SP_DOWN_16, AUIPC_SP, ADDI_SP_0, RET]),
        ],
        "entry",
        TEXT + 16,
    )
    .unwrap_err();
    assert!(
        error.contains("callee") && error.contains("sp is "),
        "{error}"
    );
}

fn jump_target(words: Vec<u32>, site: u32) -> Option<u32> {
    let bytes = executable(&[code("jumper", words)], &[]);
    let elf = oer_analysis_elf::Elf::executable(&bytes).unwrap();
    let function = elf.functions().unwrap().remove(0);
    constant_jump_target(&elf, &function, site)
}

#[test]
fn a_constant_jump_names_its_target_unless_something_else_lands_on_it() {
    let words = vec![LUI_T1_0X50014000, ADDI_T1_MINUS_0X300, JR_T1];
    assert_eq!(jump_target(words, TEXT + 8), Some(0x5001_3d00));
    // A branch onto the `addi` reaches the jump without the `lui`.
    let words = vec![BEQ_TO_PLUS_8, LUI_T1_0X50014000, ADDI_T1_MINUS_0X300, JR_T1];
    assert_eq!(jump_target(words, TEXT + 12), None);
    // Only a `lui`/`addi` pair into the jump's own register is a constant.
    assert_eq!(jump_target(vec![NOP, NOP, JR_T1], TEXT + 8), None);
}

/// A stage two whose `entry` is `TEXT + 4 * words` and installs `top`
/// (the `RODATA` label) as its stack pointer.
fn stage_two(words: u32) -> tempfile::NamedTempFile {
    let entry = TEXT + 4 * words;
    let pages = (RODATA as i32 - entry as i32) >> 12;
    assert_eq!(pages << 12, RODATA as i32 - entry as i32, "page aligned");
    let auipc = ((pages as u32) << 12) | (2 << 7) | 0x17;
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        executable(
            &[
                code("padding", vec![NOP; words as usize]),
                code("entry", vec![auipc, ADDI_SP_0, RET]),
            ],
            &[("top", 0, 0)],
        ),
    )
    .unwrap();
    file
}

fn bootstrap() -> Image {
    // `boot` jumps to 0x5000, outside the bootstrap image.
    image(&[code("boot", vec![LUI_T1_0X5000, ADDI_T1_0, JR_T1])], &[])
}

fn boot_bound(image: &Image) -> oer_analysis_riscv_stack::Bound {
    let boot = image.symbol("boot").unwrap();
    image
        .with_bounds(|keep, control| {
            Ok(image
                .analysis
                .bound_with(boot, &image.resolutions, keep, control)?)
        })
        .unwrap()
        .0
}

#[test]
fn a_verified_jump_to_the_declared_entry_ends_the_bootstrap_path_and_any_other_stays_a_hole() {
    // Before: the jump is an unresolved indirect jump.
    let mut image = bootstrap();
    assert_eq!(boot_bound(&image).bytes, None);
    // A stage two whose entry is elsewhere: nothing provably goes there.
    let elsewhere = stage_two(0x2000);
    let error = image
        .end_path_at_stage_two("entry", elsewhere.path(), "top")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("no jump of the bootstrap provably goes there"),
        "{error}"
    );
    assert_eq!(boot_bound(&image).bytes, None);
    // The entry the jump goes to.
    let file = stage_two(0x1000);
    let handoff = image
        .end_path_at_stage_two("entry", file.path(), "top")
        .unwrap();
    assert_eq!((handoff.entry, handoff.sites.len()), (0x5000, 1));
    let bound = boot_bound(&image);
    assert_eq!((bound.bytes, bound.unresolved.len()), (Some(0), 0));
    assert!(handoff.render().contains("`entry`"));
}
