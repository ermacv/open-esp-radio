use super::*;
use oer_analysis_riscv_stack::{Assumption, Fact};

#[path = "handoff_tests.rs"]
mod handoff;
#[path = "reachability_tests.rs"]
mod reachability;

pub(crate) const SP_DOWN_16: u32 = 0xff01_0113; // addi sp, sp, -16
pub(crate) const SAVE_RA: u32 = 0x0011_2623; // sw ra, 12(sp)
pub(crate) const LOAD_RA: u32 = 0x00c1_2083; // lw ra, 12(sp)
pub(crate) const SP_UP_16: u32 = 0x0101_0113; // addi sp, sp, 16
const SP_DOWN_32: u32 = 0xfe01_0113; // addi sp, sp, -32
const SP_UP_32: u32 = 0x0201_0113; // addi sp, sp, 32
pub(crate) const RET: u32 = 0x0000_8067; // jalr zero, 0(ra)
const LOAD_SLOT: u32 = 0x0001_2783; // lw a5, 0(sp)
const CALL_A5: u32 = 0x0007_80e7; // jalr ra, 0(a5)
pub(crate) const TEXT: u32 = 0x1000;
const RODATA: u32 = 0x2000;

/// `jal ra, target` at `site`.
pub(crate) fn call(site: u32, target: u32) -> u32 {
    let offset = target.wrapping_sub(site);
    ((offset >> 20 & 1) << 31)
        | ((offset >> 1 & 0x3ff) << 21)
        | ((offset >> 11 & 1) << 20)
        | ((offset >> 12 & 0xff) << 12)
        | (1 << 7)
        | 0x6f
}

/// A code symbol: its words and its `.stack_sizes` record, if any.
pub(crate) struct Code {
    pub(crate) name: &'static str,
    pub(crate) words: Vec<u32>,
    pub(crate) frame: Option<u8>,
}

/// The test program, from `TEXT`: `runtime_main` (16 bytes) calls `leaf`
/// (32 bytes) at its full depth, so its bound is 48 bytes; `indirect` (16
/// bytes) calls through a stack slot, an unresolved site; `asm_entry` has no
/// frame record when `records` is false for it.
pub(crate) fn program(asm_record: bool) -> Vec<Code> {
    let leaf = TEXT + 4 * 6;
    vec![
        Code {
            name: "runtime_main",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, leaf),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        },
        Code {
            name: "leaf",
            words: vec![SP_DOWN_32, SP_UP_32, RET],
            frame: Some(32),
        },
        Code {
            name: "indirect",
            words: vec![
                SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
            ],
            frame: Some(16),
        },
        Code {
            name: "asm_entry",
            words: vec![RET],
            frame: asm_record.then_some(0),
        },
    ]
}

/// A static RV32 executable of `code` in `.text` at `TEXT`, with its frame
/// records in `.stack_sizes` and `data` symbols (name, offset, size; size 0
/// is a label) in a `.rodata` at `RODATA`.
fn executable(code: &[Code], data: &[(&str, u32, u32)]) -> Vec<u8> {
    executable_at(TEXT, code, data)
}

fn executable_at(text_address: u32, code: &[Code], data: &[(&str, u32, u32)]) -> Vec<u8> {
    let mut text = Vec::new();
    let mut sizes = Vec::new();
    let mut symbols = Vec::new();
    for symbol in code {
        let address = text_address + text.len() as u32;
        for word in &symbol.words {
            text.extend_from_slice(&word.to_le_bytes());
        }
        symbols.push((
            symbol.name,
            address,
            4 * symbol.words.len() as u32,
            0x12_u8,
            1_u16,
        ));
        if let Some(frame) = symbol.frame {
            sizes.extend_from_slice(&address.to_le_bytes());
            sizes.push(frame);
        }
    }
    let rodata_len = data
        .iter()
        .map(|&(_, offset, size)| offset + size)
        .max()
        .unwrap_or(0)
        .max(4);
    let rodata = vec![0_u8; rodata_len as usize];
    for &(name, offset, size) in data {
        // STB_GLOBAL with STT_OBJECT, or STT_NOTYPE for a label.
        let info = if size == 0 { 0x10 } else { 0x11 };
        symbols.push((name, RODATA + offset, size, info, 6));
    }
    let mut strtab = vec![0_u8];
    let mut symtab = vec![0_u8; 16];
    for (name, address, size, info, section) in symbols {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&address.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        symtab.push(info);
        symtab.push(0);
        symtab.extend_from_slice(&section.to_le_bytes());
    }
    let shstrtab = b"\0.text\0.stack_sizes\0.symtab\0.strtab\0.shstrtab\0.rodata\0";
    let mut out = vec![0_u8; 52 + 2 * 32];
    let place = |out: &mut Vec<u8>, bytes: &[u8]| {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let offset = out.len() as u32;
        out.extend_from_slice(bytes);
        offset
    };
    let text_offset = place(&mut out, &text);
    let sizes_offset = place(&mut out, &sizes);
    let symtab_offset = place(&mut out, &symtab);
    let strtab_offset = place(&mut out, &strtab);
    let shstrtab_offset = place(&mut out, shstrtab);
    let rodata_offset = place(&mut out, &rodata);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let sections = out.len() as u32;
    // name, type, flags, address, offset, size, link, info, align, entry size
    let headers: [[u32; 10]; 7] = [
        [0; 10],
        [
            1,
            1,
            6,
            text_address,
            text_offset,
            text.len() as u32,
            0,
            0,
            4,
            0,
        ],
        [7, 1, 0, 0, sizes_offset, sizes.len() as u32, 0, 0, 1, 0],
        [20, 2, 0, 0, symtab_offset, symtab.len() as u32, 4, 1, 4, 16],
        [28, 3, 0, 0, strtab_offset, strtab.len() as u32, 0, 0, 1, 0],
        [
            36,
            3,
            0,
            0,
            shstrtab_offset,
            shstrtab.len() as u32,
            0,
            0,
            1,
            0,
        ],
        [46, 1, 2, RODATA, rodata_offset, rodata_len, 0, 0, 4, 0],
    ];
    for fields in headers {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    }
    // ELF header: ELFCLASS32, little endian, ET_EXEC, EM_RISCV.
    let mut elf = Vec::new();
    elf.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    elf.extend_from_slice(&2_u16.to_le_bytes());
    elf.extend_from_slice(&243_u16.to_le_bytes());
    for word in [1_u32, text_address, 52, sections, 0] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52_u16, 32, 2, 40, 7, 5] {
        elf.extend_from_slice(&half.to_le_bytes());
    }
    for (offset, address, size, flags) in [
        (text_offset, text_address, text.len() as u32, 5_u32),
        (rodata_offset, RODATA, rodata_len, 4),
    ] {
        for word in [1_u32, offset, address, address, size, size, flags, 4] {
            elf.extend_from_slice(&word.to_le_bytes());
        }
    }
    out[..116].copy_from_slice(&elf);
    out
}

pub(crate) fn image(code: &[Code], data: &[(&str, u32, u32)]) -> Image {
    Image::new(executable(code, data), &[], &[], None).unwrap()
}

mod frame_limits {
    use super::*;
    use crate::{CheckedElf, interrupts::Required, stack::frames::FrameAudit};
    use oer_chip_profile::policy::FrameException;

    fn subject(bytes: u64) -> Image {
        let mut image = image(
            &[
                Code {
                    name: "root",
                    words: vec![RET],
                    frame: Some(0),
                },
                // No call reaches this function: the frame gate still sees it.
                Code {
                    name: "test::large::<u8>",
                    words: vec![RET],
                    frame: Some(0),
                },
            ],
            &[],
        );
        let address = image.symbol("test::large::<u8>").unwrap();
        image.stack_sizes.insert(address, bytes);
        image
    }

    fn exception(policy: &mut StackPolicy, bytes: u64) {
        policy.frame_exceptions.runtime.push(FrameException {
            function: "test::large::<u8>".into(),
            reason: "the local packet buffer".into(),
            ceiling_bytes: bytes,
        });
    }

    #[test]
    fn every_record_counts_even_if_unreachable_and_the_limit_is_inclusive() {
        let (_dir, policy) = policy_with(BASE, "");
        assert!(
            FrameAudit::of(&subject(4096), &policy, CheckedElf::Runtime)
                .check(Required::Product)
                .is_ok()
        );
        let error = FrameAudit::of(&subject(4097), &policy, CheckedElf::Runtime)
            .check(Required::Product)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("test::large::<u8>") && error.contains("4097"),
            "{error}"
        );
    }

    #[test]
    fn the_policy_supplies_the_limit_and_missing_records_belong_to_coverage() {
        let (_dir, mut policy) = policy_with(BASE, "");
        policy.max_move_bytes = 16;
        let mut image = subject(17);
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Runtime)
                .check(Required::Product)
                .is_err()
        );
        image.stack_sizes.clear();
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Runtime)
                .check(Required::Product)
                .is_ok()
        );
    }

    #[test]
    fn an_exception_holds_the_frame_to_its_ceiling_in_both_classes() {
        let (_dir, mut policy) = policy_with(BASE, "");
        exception(&mut policy, 8192);
        for required in [Required::Product, Required::Diagnostic] {
            for bytes in [8191, 8192] {
                let audit = FrameAudit::of(&subject(bytes), &policy, CheckedElf::Runtime);
                assert!(audit.check(required).unwrap().is_empty());
                assert!(
                    audit
                        .render()
                        .contains(&format!("{bytes}\t8192\t{}", 8192 - bytes))
                );
            }
            let error = FrameAudit::of(&subject(8193), &policy, CheckedElf::Runtime)
                .check(required)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("8193") && error.contains("ceiling of 8192"),
                "{error}"
            );
        }
    }

    #[test]
    fn missing_and_fixed_functions_make_exceptions_stale_even_in_diagnostics() {
        let (_dir, mut policy) = policy_with(BASE, "");
        exception(&mut policy, 8192);
        for required in [Required::Product, Required::Diagnostic] {
            for bytes in [0, 4096] {
                let error = FrameAudit::of(&subject(bytes), &policy, CheckedElf::Runtime)
                    .check(required)
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("stale frame exception"), "{error}");
            }
            // A different generic instantiation cannot keep the exception alive.
            let mut image = subject(8192);
            image.functions[1].names = vec!["test::large::<u16>".into()];
            let error = FrameAudit::of(&image, &policy, CheckedElf::Runtime)
                .check(required)
                .unwrap_err()
                .to_string();
            assert!(error.contains("matches 0 functions"), "{error}");
        }
    }

    #[test]
    fn matching_two_implementations_fails_but_aliases_count_once() {
        let (_dir, mut policy) = policy_with(BASE, "");
        exception(&mut policy, 8192);
        let mut image = subject(8192);
        image.functions[1].names.push("test::large::<u8>".into());
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Runtime)
                .check(Required::Product)
                .is_ok()
        );
        image.functions[0].names.push("test::large::<u8>".into());
        let error = FrameAudit::of(&image, &policy, CheckedElf::Runtime)
            .check(Required::Product)
            .unwrap_err()
            .to_string();
        assert!(error.contains("matches 2 functions"), "{error}");
    }

    #[test]
    fn rust_hashes_do_not_distinguish_implementations_in_exception_keys() {
        let (_dir, mut policy) = policy_with(BASE, "");
        exception(&mut policy, 8192);
        policy.frame_exceptions.runtime[0].function = "test::example".into();
        let mut image = subject(8192);
        image.functions[1].names = vec!["_ZN4test7example17h0123456789abcdefE".into()];
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Runtime)
                .check(Required::Product)
                .is_ok()
        );
        image.functions[0].names = vec!["_ZN4test7example17hfedcba9876543210E".into()];
        let error = FrameAudit::of(&image, &policy, CheckedElf::Runtime)
            .check(Required::Product)
            .unwrap_err()
            .to_string();
        assert!(error.contains("matches 2 functions"), "{error}");
    }

    #[test]
    fn diagnostic_warns_and_bootstrap_uses_its_own_exceptions() {
        let (_dir, mut policy) = policy_with(BASE, "");
        let image = subject(8192);
        for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
            let audit = FrameAudit::of(&image, &policy, elf);
            assert!(audit.check(Required::Product).is_err());
            let warnings = audit.check(Required::Diagnostic).unwrap();
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("test::large::<u8>"));
            let output = tempfile::tempdir().unwrap();
            crate::stack::frames::audit(&image, &policy, elf, Required::Diagnostic, output.path())
                .unwrap();
            let report = match elf {
                CheckedElf::Runtime => crate::stack::frames::RUNTIME_REPORT,
                CheckedElf::Bootstrap => crate::stack::frames::BOOTSTRAP_REPORT,
            };
            assert!(
                std::fs::read_to_string(output.path().join(report))
                    .unwrap()
                    .contains("warning: frame `test::large::<u8>`")
            );
        }
        exception(&mut policy, 8192);
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Bootstrap)
                .check(Required::Product)
                .is_err()
        );
        policy.frame_exceptions.bootstrap = std::mem::take(&mut policy.frame_exceptions.runtime);
        assert!(
            FrameAudit::of(&image, &policy, CheckedElf::Bootstrap)
                .check(Required::Product)
                .unwrap()
                .is_empty()
        );
    }
}

const BASE: &str = r#"
schema = 6
coverage_policy = "coverage.toml"
max_move_bytes = 4096

[core.0.task_stack]
root = "runtime_main"
storage_symbol = "boot_stack"
minimum_free_bytes = 512

[core.0.interrupt_stack]

[bootstrap_stack]
root = "runtime_main"
bottom_symbol = "stack_bottom"
top_symbol = "stack_top"
minimum_free_bytes = 64
"#;

const ASM_REVIEW: &str = r#"
[[reviewed]]
symbols = ["asm_entry"]
category = "assembly"
source = "entry.rs"
reason = "naked entry"
"#;

/// A policy directory: `stack.toml` from `policy` and `coverage.toml` with
/// the `reviews`.
pub(crate) fn policy_with(policy: &str, reviews: &str) -> (tempfile::TempDir, StackPolicy) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("stack.toml"), policy).unwrap();
    let reviews = if reviews.is_empty() {
        "reviewed = []\n"
    } else {
        reviews
    };
    std::fs::write(
        directory.path().join("coverage.toml"),
        format!("schema = 1\n{reviews}"),
    )
    .unwrap();
    let loaded = StackPolicy::load(&directory.path().join("stack.toml")).unwrap();
    (directory, loaded)
}

#[test]
fn bootstrap_exports_a_rom_summary_the_runtime_does_not_apply_and_its_coverage_reviews() {
    use crate::{Check, CheckInput, Gates};

    const ROM: u32 = 0x3000;
    let rom = executable_at(
        ROM,
        &[Code {
            name: "rom_dispatch",
            words: vec![LOAD_SLOT, CALL_A5, RET],
            frame: Some(0),
        }],
        &[],
    );
    let summaries = [Summary {
        name: "rom_dispatch".into(),
        address: ROM,
        frame: 0,
        calls: vec![],
    }];
    let runtime = Image::new(
        executable(
            &[Code {
                name: "runtime_main",
                words: vec![RET],
                frame: Some(0),
            }],
            &[],
        ),
        &[&rom],
        &summaries,
        None,
    )
    .expect("the synthetic runtime is valid");
    assert!(runtime.analysis.summaries.is_empty());

    let bootstrap = executable(
        &[
            Code {
                name: "runtime_main",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    call(TEXT + 8, ROM),
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                ],
                frame: Some(16),
            },
            Code {
                name: "asm_entry",
                words: vec![RET],
                frame: None,
            },
        ],
        &[("stack_bottom", 0, 0), ("stack_top", 0x100, 0)],
    );
    let chip = tempfile::tempdir().unwrap();
    let layer = chip.path().join("chips/esp32s31/platform/chip.toml");
    std::fs::create_dir_all(layer.parent().unwrap()).unwrap();
    std::fs::write(
        &layer,
        r#"
schema = 2
id = "esp32s31"
family = "espressif"
isa = "rv32imac"
rust-target = "riscv32imac-unknown-none-elf"
boot = "staged"
espflash-chip = "esp32s31"
revisions = ["test"]
[properties]
wifi-bands = []
bluetooth = []
ieee802154 = false
cores = 1
[memory]
internal-sram = { origin = 0x1000, length = 0x2000 }
flash-xip = { origin = 0x10000, length = 0x1000 }
retained = { origin = 0x20000, length = 0x100 }
task-stacks = "sram"
[memory.attributes]
sram = { executable = true, cached = false, dma = true, retained = false, writable = true }
retained = { executable = true, cached = false, dma = false, retained = true, writable = true }
flash-xip = { executable = true, cached = true, dma = false, retained = false, writable = false }
"#,
    )
    .unwrap();
    let profile = oer_chip_profile::Profile::load(chip.path(), "esp32s31")
        .expect("the synthetic chip profile is valid");
    let (directory, policy) = policy_with(BASE, ASM_REVIEW);
    let input = CheckInput {
        root: directory.path(),
        profile: &profile,
        policy: &policy,
        elf: &directory.path().join("bootstrap.elf"),
        flat: None,
        output: directory.path(),
        task_stack_proven: false,
        stage_two_elf: None,
        panics: None,
    };
    for checks in [&[Check::Stack][..], &[Check::Frames][..]] {
        let image = Image::new(bootstrap.clone(), &[&rom], &summaries, None)
            .expect("the synthetic bootstrap is valid");
        let outcome = Gates::of(checks)
            .bootstrap_image(&input, image)
            .expect("the bootstrap checks run");
        assert!(outcome.results.iter().all(|(_, result)| result.passes()));
        assert_eq!(outcome.rom_summaries, ["rom_dispatch".to_owned()].into());
        // Only the stack check applies the coverage review.
        assert_eq!(
            outcome.coverage_reviews.is_empty(),
            !checks.contains(&Check::Stack)
        );
    }
}

fn audit(
    policy: &StackPolicy,
    image: &Image,
    stack: impl Fn(&StackPolicy) -> &TaskStack,
) -> Result<StackAudit> {
    let mut audit = StackAudit::of(
        image,
        policy,
        &[oer_chip_profile::policy::StackRoot {
            hart: Some(0),
            name: "test stack".into(),
            stack: stack(policy),
        }],
    )?;
    // This fixture declares hart zero as its boot hart without a chip contract.
    audit.boot_hart = Some(0);
    Ok(audit)
}

#[test]
fn the_policy_names_each_stack_root_and_its_reserve() {
    let (directory, base) = policy_with(BASE, ASM_REVIEW);
    assert_eq!(
        base.coverage_policy.as_deref(),
        Some(directory.path().join("coverage.toml").as_path())
    );
    assert_eq!(
        base.task_stack(0).unwrap().storage(),
        Storage::Symbol("boot_stack")
    );
    assert_eq!(
        base.bootstrap_stack.as_ref().unwrap().storage(),
        Storage::Range {
            bottom: "stack_bottom",
            top: "stack_top"
        }
    );
    assert_eq!(
        base.runtime_environment(),
        [(
            format!("OPEN_RADIO_TASK_STACK_HART{}_MINIMUM_FREE_BYTES", 0),
            "512".to_owned()
        ),]
    );
    // An extension reuses its base unchanged.
    let nested = directory.path().join("hil");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(
        nested.join("stack.toml"),
        "schema = 6\nextends = \"../stack.toml\"\n",
    )
    .unwrap();
    let extended = StackPolicy::load(&nested.join("stack.toml")).unwrap();
    assert_eq!(
        extended
            .coverage_policy
            .as_ref()
            .unwrap()
            .canonicalize()
            .unwrap(),
        base.coverage_policy
            .as_ref()
            .unwrap()
            .canonicalize()
            .unwrap()
    );
    assert_eq!(extended.runtime_task_stacks().len(), 1);
    assert_eq!(extended.runtime_environment(), base.runtime_environment());
    // An extension adds nothing of its own.
    std::fs::write(
        nested.join("stack.toml"),
        "schema = 6\nextends = \"../stack.toml\"\nmax_move_bytes = 1\n",
    )
    .unwrap();
    assert!(StackPolicy::load(&nested.join("stack.toml")).is_err());
}

#[test]
fn a_policy_out_of_its_schema_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stack.toml");
    let load = |source: String| {
        std::fs::write(&path, source).unwrap();
        StackPolicy::load(&path).map(|_| ())
    };
    // The per-frame rules are gone: their fields are unknown.
    assert!(load(format!("{BASE}warn_frame_bytes = 8192\n")).is_err());
    assert!(
        load(format!(
            "{BASE}\n[[reviewed_frames]]\nfunction_contains = \"x\"\n"
        ))
        .is_err()
    );
    assert!(load(BASE.replace("schema = 6", "schema = 4")).is_err());
    // One storage form per stack.
    assert!(
        load(BASE.replace(
            "storage_symbol = \"boot_stack\"",
            "storage_symbol = \"boot_stack\"\nbottom_symbol = \"b\"\ntop_symbol = \"t\""
        ))
        .is_err()
    );
    assert!(load(BASE.replace("bottom_symbol = \"stack_bottom\"\n", "")).is_err());
    assert!(
        load(BASE.replace("root = \"runtime_main\"\nstorage", "root = \"\"\nstorage")).is_err()
    );
    assert!(load(BASE.replace("minimum_free_bytes = 512", "minimum_free_bytes = 0")).is_err());
    assert!(
        load(BASE.replace(
            "minimum_free_bytes = 512",
            "minimum_free_bytes = 512\nunknown = true"
        ))
        .is_err()
    );
    assert!(load(BASE.to_owned()).is_ok());
    // A policy of the move limit alone names no stacks; one that names
    // some names all of them.
    let compiler = StackPolicy::load({
        std::fs::write(&path, "schema = 6\nmax_move_bytes = 4096\n").unwrap();
        &path
    })
    .unwrap();
    assert!(compiler.stacks().is_err());
    assert!(compiler.runtime_environment().is_empty());
    assert!(
        load("schema = 6\nmax_move_bytes = 4096\ncoverage_policy = \"c.toml\"\n".into()).is_err()
    );
    // An extension may not extend an extension.
    let base = directory.path().join("base.toml");
    std::fs::write(&base, "schema = 6\nextends = \"stack.toml\"\n").unwrap();
    assert!(load("schema = 6\nextends = \"base.toml\"\n".to_owned()).is_err());
}

#[test]
fn a_bound_within_its_budget_passes_with_its_headroom_reported() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    // 1024 bytes less 512 free: a 512-byte budget for a 48-byte bound.
    let image = image(&program(false), &[("boot_stack", 0, 1024)]);
    let audit = audit(&policy, &image, |policy| policy.task_stack(0).unwrap()).unwrap();
    assert_eq!(audit.check().unwrap(), Vec::<String>::new());
    let report = audit.render();
    let usage = budget::header(&image.resources, image.resources.usage.peak_admitted_bytes);
    assert!(image.resources.usage.work_units > 0 && image.resources.usage.peak_admitted_bytes > 0);
    assert!(report.starts_with(&usage), "{report}");
    assert!(
        report.contains("`runtime_main` needs 48 bytes, proven"),
        "{report}"
    );
    assert!(report.contains("budget 512 bytes"), "{report}");
    assert!(report.contains("464 bytes of headroom"), "{report}");
    assert!(
        report.contains("    16 runtime_main\n        32 leaf"),
        "{report}"
    );
}

#[test]
fn a_bound_over_its_budget_fails() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let fits = image(&program(false), &[("boot_stack", 0, 512 + 48)]);
    audit(&policy, &fits, |policy| policy.task_stack(0).unwrap())
        .unwrap()
        .check()
        .unwrap();
    let over = image(&program(false), &[("boot_stack", 0, 512 + 47)]);
    let error = audit(&policy, &over, |policy| policy.task_stack(0).unwrap())
        .unwrap()
        .check()
        .unwrap_err()
        .to_string();
    assert!(error.contains("needs 48 bytes"), "{error}");
    assert!(error.contains("47-byte budget"), "{error}");
    // Storage that cannot keep its reserve fails whatever the bound.
    let small = image(&program(false), &[("boot_stack", 0, 512)]);
    assert!(
        audit(&policy, &small, |policy| policy.task_stack(0).unwrap())
            .unwrap()
            .check()
            .is_err()
    );
}

#[test]
fn a_root_the_image_lacks_fails() {
    let (_directory, policy) = policy_with(
        &BASE.replace("\"runtime_main\"\nstorage", "\"absent\"\nstorage"),
        ASM_REVIEW,
    );
    let image = image(&program(false), &[("boot_stack", 0, 1024)]);
    let error = audit(&policy, &image, |policy| policy.task_stack(0).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("has no `absent`"), "{error}");
}

#[test]
fn a_stack_the_image_does_not_link_fails() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let unlinked = image_without_storage();
    let error = audit(&policy, &unlinked, |policy| policy.task_stack(0).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("links no test stack"), "{error}");
}

fn image_without_storage() -> Image {
    image(&program(false), &[])
}

#[test]
fn a_partial_boot_hart_bound_warns_with_its_unresolved_sites_reported() {
    let policy_source = BASE.replace("\"runtime_main\"\nstorage", "\"indirect\"\nstorage");
    let (_directory, policy) = policy_with(&policy_source, ASM_REVIEW);
    let image = image(&program(false), &[("boot_stack", 0, 1024)]);
    let audit = audit(&policy, &image, |policy| policy.task_stack(0).unwrap()).unwrap();
    let warnings = audit.check().unwrap();
    assert!(warnings.iter().any(|warning| warning.contains("16 + ?")));
    let report = audit.render();
    assert!(report.contains("needs 16 + ? bytes, partial"), "{report}");
    let site = TEXT + 4 * (6 + 3 + 3);
    assert!(
        report.contains(&format!(
            "indirect call through a stack slot at {site:#010x} in indirect+0xc"
        )),
        "{report}"
    );
    // The part a partial bound proves must still fit.
    let over = image_with_stack(512 + 15);
    let error = audit_of(&policy, &over).check().unwrap_err().to_string();
    assert!(error.contains("16 + ? bytes"), "{error}");
}

#[test]
fn the_fact_boot_identity_selects_which_partial_task_bound_can_pass() {
    let partial = BASE.replace("\"runtime_main\"\nstorage", "\"indirect\"\nstorage");
    let (_directory, policy) = policy_with(&partial, ASM_REVIEW);
    for harts in [3, 7] {
        let boot = harts - 1;
        let mut contract =
            Contract::of(&oer_chip_profile::Profile::tracked("esp32s31").unwrap()).unwrap();
        contract.interrupts.harts = (0..harts).collect();
        contract.interrupts.boot_hart = boot;
        let mut image = image_with_stack(1024);
        image.contract = Some(contract);
        for hart in 0..harts {
            let root = oer_chip_profile::policy::StackRoot {
                hart: Some(hart),
                name: format!("hart {hart} task stack"),
                stack: policy.task_stack(0).unwrap(),
            };
            let mut audit = StackAudit::of(&image, &policy, &[root]).unwrap();
            assert_eq!(audit.check().is_ok(), hart == boot);
            audit.declared_proven = true;
            assert!(audit.check().is_err());
        }
    }
}

#[test]
fn a_stack_declared_proven_fails_on_an_unresolved_path_and_otherwise_passes() {
    let partial = BASE.replace("\"runtime_main\"\nstorage", "\"indirect\"\nstorage");
    let (_directory, partial_policy) = policy_with(&partial, ASM_REVIEW);
    let image = image_with_stack(1024);
    let mut audit = audit_of(&partial_policy, &image);
    audit.declared_proven = true;
    let error = audit.check().unwrap_err().to_string();
    assert!(
        error.contains("image declares task-stack proven"),
        "{error}"
    );
    let site = TEXT + 4 * (6 + 3 + 3);
    assert!(
        error.contains(&format!("{site:#010x} in indirect+0xc")),
        "{error}"
    );
    // Without the declaration, the boot hart's partial bound is reported.
    audit.declared_proven = false;
    assert!(!audit.check().unwrap().is_empty());
    // A fully resolved bound passes, and the report says it is declared.
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let mut full = audit_of(&policy, &image);
    full.declared_proven = true;
    assert_eq!(full.check().unwrap(), Vec::<String>::new());
    assert!(
        full.render()
            .contains("the image sizes them from their bound")
    );
    assert!(
        !audit_of(&policy, &image)
            .render()
            .contains("sizes them from their bound")
    );
}

/// The `indirect` root of [`program`] with its stack-slot call resolved to
/// `leaf` by `fact`.
fn resolved_through(fact: Fact) -> (tempfile::TempDir, StackPolicy, Image) {
    let source = BASE.replace("\"runtime_main\"\nstorage", "\"indirect\"\nstorage");
    let (directory, policy) = policy_with(&source, ASM_REVIEW);
    let mut image = image_with_stack(1024);
    let site = TEXT + 4 * (6 + 3 + 3);
    image.resolutions.add(site, fact, [TEXT + 4 * 6]);
    (directory, policy, image)
}

#[test]
fn a_bound_is_proven_only_without_an_assumption() {
    let (_directory, policy, image) = resolved_through(Fact::WakerVtables);
    let mut audit = audit_of(&policy, &image);
    audit
        .size_sources
        .insert(0, crate::linker::task_stack::SizeSource::Bound);
    let report = audit.render();
    assert!(
        report.contains("`indirect` needs 48 bytes, proven;"),
        "{report}"
    );
    assert!(
        report.contains("allocated from the proven bound plus"),
        "{report}"
    );
    let (_directory, policy, image) = resolved_through(Fact::FieldType);
    let mut audit = audit_of(&policy, &image);
    audit
        .size_sources
        .insert(0, crate::linker::task_stack::SizeSource::Bound);
    let report = audit.render();
    assert!(
        report.contains("`indirect` needs 48 bytes, conditional (ExecutorInvariant);"),
        "{report}"
    );
    assert!(
        report.contains("allocated from the conditional (ExecutorInvariant) bound plus"),
        "{report}"
    );
    assert!(!report.contains("proven"), "{report}");
    // The gate is unchanged: the admitted assumption passes with a warning.
    assert_eq!(audit.check().unwrap().len(), 1);
}

#[test]
fn the_site_report_lists_each_taken_site_with_its_fact_and_assumption() {
    let site = TEXT + 4 * (6 + 3 + 3);
    for (fact, kind, assumptions) in [
        (
            Fact::FieldType,
            sites::Kind::FieldType,
            vec![Assumption::ExecutorInvariant],
        ),
        // The waker vtable census is complete: its sites are proven.
        (Fact::WakerVtables, sites::Kind::WakerVtables, vec![]),
    ] {
        let (_directory, policy, image) = resolved_through(fact);
        let audit = audit_of(&policy, &image);
        let mut report = sites::Report::default();
        for (label, bound) in audit.roots(&policy) {
            report.add(&image, &label, bound);
        }
        let [root] = &report.roots[..] else {
            panic!("one root: {report:?}");
        };
        assert_eq!(root.label, format!("CPU{}", 0));
        assert_eq!(root.root, "indirect");
        assert_eq!(
            root.sites,
            [sites::Site {
                site,
                function: "indirect".to_owned(),
                offset: 0xc,
                targets: 1,
                facts: vec![kind],
                assumptions: assumptions.clone(),
            }]
        );
        assert_eq!(report.totals.sites, 1);
        assert_eq!(report.totals.by_fact, BTreeMap::from([(kind, 1)]));
        assert_eq!(report.totals.proven, usize::from(assumptions.is_empty()));
        let text = report.render();
        let assumption = if assumptions.is_empty() {
            "none"
        } else {
            "ExecutorInvariant"
        };
        assert!(
            text.contains(&format!(
                "  {site:#010x}\t1\t{kind:?}\t{assumption}\tindirect+0xc\n"
            )),
            "{text}"
        );
        assert!(
            text.contains(&format!("totals: 1 distinct sites: {kind:?} 1\n")),
            "{text}"
        );
        let json: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json["roots"][0]["sites"][0]["facts"][0],
            format!("{kind:?}")
        );
    }
    // A site the machine code resolves by itself is not a fact's site.
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let image = image_with_stack(1024);
    let audit = audit_of(&policy, &image);
    let mut report = sites::Report::default();
    for (label, bound) in audit.roots(&policy) {
        report.add(&image, &label, bound);
    }
    assert!(report.roots[0].sites.is_empty(), "{report:?}");
}

fn image_with_stack(bytes: u32) -> Image {
    image(&program(false), &[("boot_stack", 0, bytes)])
}

fn audit_of(policy: &StackPolicy, image: &Image) -> StackAudit {
    audit(policy, image, |policy| policy.task_stack(0).unwrap()).unwrap()
}

#[test]
fn stack_report_counts_functions_with_expression_limits() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let mut image = image_with_stack(256);
    assert_eq!(image.analysis.expression_limited_functions(), 0);
    assert!(
        !audit_of(&policy, &image)
            .render()
            .contains("expression limits:")
    );
    image
        .analysis
        .functions
        .values_mut()
        .next()
        .unwrap()
        .expression_limited = true;
    assert_eq!(image.analysis.expression_limited_functions(), 1);
    assert!(
        audit_of(&policy, &image)
            .render()
            .contains("expression limits: 1 functions")
    );
}

#[test]
fn a_range_stack_takes_its_capacity_from_its_bounds() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let image = image(
        &program(false),
        &[("stack_bottom", 0, 0), ("stack_top", 0x100, 0)],
    );
    let audit = audit(&policy, &image, |policy| {
        policy.bootstrap_stack.as_ref().unwrap()
    })
    .unwrap();
    audit.check().unwrap();
    assert!(
        audit
            .render()
            .contains("budget 192 bytes (256 bytes of `stack_bottom`..`stack_top` less 64")
    );
    // One bound without the other is a broken layout.
    let broken = image_one_bound();
    assert!(audit_bootstrap(&policy, &broken).is_err());
}

fn image_one_bound() -> Image {
    image(&program(false), &[("stack_bottom", 0, 0)])
}

fn audit_bootstrap(policy: &StackPolicy, image: &Image) -> Result<StackAudit> {
    audit(policy, image, |policy| {
        policy.bootstrap_stack.as_ref().unwrap()
    })
}

#[test]
fn an_image_without_frame_records_fails() {
    let (_directory, policy) = policy_with(BASE, ASM_REVIEW);
    let mut code = program(true);
    for symbol in &mut code {
        symbol.frame = None;
    }
    let image = image(&code, &[("boot_stack", 0, 1024)]);
    let error = audit(&policy, &image, |policy| policy.task_stack(0).unwrap())
        .unwrap_err()
        .to_string();
    assert!(error.contains("-Z emit-stack-sizes"), "{error}");
}

#[test]
fn every_function_without_a_frame_record_needs_one_review() {
    let image = image(&program(false), &[("boot_stack", 0, 1024)]);
    let (_directory, reviewed) = policy_with(BASE, ASM_REVIEW);
    let audit_reviewed = audit_of(&reviewed, &image);
    audit_reviewed.check().unwrap();
    assert_eq!(audit_reviewed.coverage.functions, 4);
    assert_eq!(audit_reviewed.coverage.measured, 3);
    assert!(audit_reviewed.render().contains(&format!(
        "reviewed Assembly {:#010x} asm_entry",
        TEXT + 4 * 16
    )));
    let (_directory, unreviewed) = policy_with(BASE, "");
    let error = audit_of(&unreviewed, &image)
        .check()
        .unwrap_err()
        .to_string();
    assert!(error.contains("asm_entry at"), "{error}");
    // A recorded function needs no review, and its review applies to nothing.
    assert_eq!(
        audit_reviewed.coverage.applied,
        ["asm_entry".to_owned()].into()
    );
    let (_directory, unreviewed) = policy_with(BASE, "");
    audit_of(&unreviewed, &image_with_records())
        .check()
        .unwrap();
    assert!(
        audit_of(&reviewed, &image_with_records())
            .coverage
            .applied
            .is_empty()
    );
    // Two reviews of one symbol are refused.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("coverage.toml");
    std::fs::write(&path, format!("schema = 1\n{ASM_REVIEW}{ASM_REVIEW}")).unwrap();
    assert!(CoveragePolicy::load(&path).is_err());
    std::fs::write(
        &path,
        format!("schema = 1\n{ASM_REVIEW}").replace("\"assembly\"", "\"any-rust\""),
    )
    .unwrap();
    assert!(CoveragePolicy::load(&path).is_err());
}

fn image_with_records() -> Image {
    image(&program(true), &[("boot_stack", 0, 1024)])
}

#[test]
fn reviews_match_without_crate_disambiguators_only() {
    use coverage::without_crate_disambiguators as stable;
    assert_eq!(
        stable("hal[1234567890abcdef]::entry::<app[abcdef1234567890]::main::{closure#0}>"),
        "hal::entry::<app::main::{closure#0}>"
    );
    assert_eq!(
        stable(
            "esp_hal[c21304bb353fc7d6]::soc::start_core1_init::<oer_chip_a_hil_agent[bd01774463a7ff5]::runtime_main::{closure#0}>"
        ),
        "esp_hal::soc::start_core1_init::<oer_chip_a_hil_agent::runtime_main::{closure#0}>"
    );
    assert_eq!(stable("crate[0]::entry"), "crate::entry");
    for name in [
        "hal::entry_impl::<A>",
        "hal::entry::<[u8; 16]>",
        "hal[not-a-crate-hash]::entry::<B>",
        "hal[]::entry",
        "hal[1234567890abcdef0]::entry",
        "hal::entry::<[1234]>",
    ] {
        assert_eq!(stable(name), name);
    }
}

#[test]
fn the_repository_policies_load() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, text) in oer_espressif_facts::tracked::FILES.iter().copied().chain([
        (
            "chips/esp32s31/platform/stack.toml",
            include_str!("../../../../../chips/esp32s31/platform/stack.toml"),
        ),
        (
            "chips/espressif/staged-layout/stack-coverage.toml",
            include_str!("../../../../../chips/espressif/staged-layout/stack-coverage.toml"),
        ),
    ]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    for chip in oer_chip_profile::Profile::all(&root).unwrap() {
        let policy = StackPolicy::load(&root.join(chip.stack_policy())).unwrap();
        if let Some(coverage) = &policy.coverage_policy {
            CoveragePolicy::load(coverage).unwrap();
        }
    }
}

mod cores {
    use super::*;
    use crate::stack::cores;

    const POLICY: &str = r#"
schema = 6
coverage_policy = "coverage.toml"
max_move_bytes = 4096

[core.0.task_stack]
root = "runtime_main"
storage_symbol = "boot_stack"
minimum_free_bytes = 16

[core.0.interrupt_stack]

[core.1.task_stack]
unused = true

[core.1.interrupt_stack]
unused = true
"#;

    const USED: &str = r#"
schema = 6
coverage_policy = "coverage.toml"
max_move_bytes = 4096

[core.0.task_stack]
root = "runtime_main"
storage_symbol = "boot_stack"
minimum_free_bytes = 16

[core.0.interrupt_stack]

[core.1.task_stack]
root = "ROOT"
storage_symbol = "secondary_stack"
minimum_free_bytes = 16

[core.1.interrupt_stack]
"#;

    fn chip() -> oer_chip_profile::Profile {
        oer_chip_profile::Profile::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."),
            "esp32s31",
        )
        .unwrap()
    }

    fn starter(name: &'static str) -> Code {
        Code {
            name,
            words: vec![RET],
            frame: Some(0),
        }
    }

    fn with(extra: Option<Code>) -> Vec<Code> {
        let mut code = program(true);
        code.extend(extra);
        code
    }

    #[test]
    fn an_image_that_links_no_starter_proves_core_one_unused() {
        let (_directory, policy) = policy_with(POLICY, "");
        let image = image(&with(None), &[("boot_stack", 0, 1024)]);
        let report = cores::audit_runtime(&image, &policy, &chip()).unwrap();
        assert!(report.contains("1\tunused"), "{report}");
        // An empty linked range is an explicit 0.
        let hart = 1;
        let bottom = format!("__runtime_cpu{hart}_irq_stack_bottom");
        let top = format!("__runtime_cpu{hart}_irq_stack_top");
        let empty = self::image(
            &with(None),
            &[
                ("boot_stack", 0, 1024),
                (&bottom, 0x800, 0),
                (&top, 0x800, 0),
            ],
        );
        cores::audit_runtime(&empty, &policy, &chip()).unwrap();
        cores::audit_bootstrap(&empty, &chip()).unwrap();
    }

    #[test]
    fn a_linked_starter_with_core_one_unused_fails() {
        let (_directory, policy) = policy_with(POLICY, "");
        for name in [
            "oer_esp32s31_hal::cores::Cores::start_hart1::<closure>",
            "<oer_esp32s31_hal::cores::Cores>::start_hart1",
            "oer_esp32s31_hal::cores::Cores::start_hart1",
        ] {
            let image = image(&with(Some(starter(name))), &[("boot_stack", 0, 1024)]);
            let error = cores::audit_runtime(&image, &policy, &chip())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("declares the task stack of core 1 unused"),
                "{name}: {error}"
            );
            assert!(error.contains(name), "{error}");
            assert!(cores::audit_bootstrap(&image, &chip()).is_err());
        }
        // A function that only resembles a starter is none.
        let image = image(
            &with(Some(starter("oer_esp32s31_hal::cores::Cores::park"))),
            &[("boot_stack", 0, 1024)],
        );
        cores::audit_runtime(&image, &policy, &chip()).unwrap();
    }

    #[test]
    fn storage_linked_for_an_unused_stack_fails() {
        let (_directory, policy) = policy_with(POLICY, "");
        let hart = 1;
        for kind in ["irq", "task"] {
            let bottom = format!("__runtime_cpu{hart}_{kind}_stack_bottom");
            let top = format!("__runtime_cpu{hart}_{kind}_stack_top");
            let image = image(
                &with(None),
                &[
                    ("boot_stack", 0, 1024),
                    (&bottom, 0x800, 0),
                    (&top, 0x900, 0),
                ],
            );
            let error = cores::audit_runtime(&image, &policy, &chip())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("unused") && error.contains("256 bytes"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_policy_that_omits_a_core_of_the_chip_fails() {
        let one_core = POLICY.split("[core.1").next().unwrap();
        let (_directory, policy) = policy_with(one_core, "");
        let image = image(&with(None), &[("boot_stack", 0, 1024)]);
        let error = cores::audit_runtime(&image, &policy, &chip())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("names 1 cores but the chip has 2"),
            "{error}"
        );
    }

    #[test]
    fn a_used_core_runs_only_with_a_proven_bound() {
        // `leaf`-rooted core 1 is proven; `indirect`-rooted is `+ ?`.
        for (root, proven) in [("leaf", true), ("indirect", false)] {
            let (_directory, policy) = policy_with(&USED.replace("ROOT", root), ASM_REVIEW);
            let image = image(
                &with(Some(starter("oer_esp32s31_hal::cores::Cores::start_hart1"))),
                &[("boot_stack", 0, 1024), ("secondary_stack", 0x400, 1024)],
            );
            cores::audit_runtime(&image, &policy, &chip()).unwrap();
            let audit = StackAudit::of(&image, &policy, &policy.runtime_task_stacks()).unwrap();
            assert_eq!(audit.stacks.len(), 2);
            let result = audit.check();
            assert_eq!(result.is_ok(), proven, "{root}: {result:?}");
            if !proven {
                let error = result.unwrap_err().to_string();
                assert!(
                    error.contains("a secondary hart runs only with a proven bound"),
                    "{error}"
                );
            }
        }
    }
}

#[test]
fn a_root_that_reaches_forbidden_rom_fails() {
    const ROM: u32 = 0x3000;
    let rom = executable_at(
        ROM,
        &[
            Code {
                name: "rom_allowed",
                words: vec![RET],
                frame: Some(0),
            },
            Code {
                name: "rom_radio",
                words: vec![RET],
                frame: Some(0),
            },
        ],
        &[],
    );
    let calling = |target: u32| {
        Image::new(
            executable(
                &[Code {
                    name: "runtime_main",
                    words: vec![
                        SP_DOWN_16,
                        SAVE_RA,
                        call(TEXT + 8, target),
                        LOAD_RA,
                        SP_UP_16,
                        RET,
                    ],
                    frame: Some(16),
                }],
                &[("boot_stack", 0, 1024)],
            ),
            &[&rom],
            &[],
            None,
        )
        .expect("the synthetic image is valid")
    };
    let forbidden = [oer_espressif_facts::RomRange {
        name: "radio".into(),
        start: ROM + 4,
        end: ROM + 8,
    }];
    let (_directory, policy) = policy_with(BASE, "");
    let allowed = calling(ROM);
    let audit = audit_of(&policy, &allowed);
    rom::audit_forbidden(&allowed, &forbidden, audit.roots(&policy)).unwrap();
    let radio = calling(ROM + 4);
    let audit = audit_of(&policy, &radio);
    let error = rom::audit_forbidden(&radio, &forbidden, audit.roots(&policy))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("CPU{} reaches forbidden ROM `radio`", 0))
            && error.contains("runtime_main -> rom_radio"),
        "{error}"
    );
}

#[test]
fn every_hart_has_its_own_root_bound_and_nonoverlapping_task_storage() {
    for harts in [1, 3, 7] {
        let mut source =
            String::from("schema=6\ncoverage_policy='coverage.toml'\nmax_move_bytes=4096\n");
        for hart in 0..harts {
            source.push_str(&format!("[core.{hart}.task_stack]\nroot='runtime_main'\nstorage_symbol='task_{hart}'\nminimum_free_bytes=16\n[core.{hart}.interrupt_stack]\n"));
        }
        let (_directory, policy) = policy_with(&source, ASM_REVIEW);
        let names: Vec<_> = (0..harts).map(|hart| format!("task_{hart}")).collect();
        let storage: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(hart, name)| (name.as_str(), hart as u32 * 1024, 1024))
            .collect();
        let image = image(&program(false), &storage);
        let audit = StackAudit::of(&image, &policy, &policy.runtime_task_stacks()).unwrap();
        audit.check().unwrap();
        assert_eq!(audit.stacks.len(), harts as usize);
        let labels: Vec<_> = audit
            .roots(&policy)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        for hart in 0..harts {
            assert!(labels.contains(&format!("CPU{hart}")));
        }
        // Every used root is checked, including one beyond the original geometry.
        let mut missing = policy.clone();
        missing.task_stack_mut(harts - 1).unwrap().root = "missing_entry".into();
        assert!(StackAudit::of(&image, &missing, &missing.runtime_task_stacks()).is_err());
        if harts > 1 {
            let mut overlapping = storage.clone();
            overlapping.last_mut().unwrap().1 = 0;
            let image = self::image(&program(false), &overlapping);
            let error = StackAudit::of(&image, &policy, &policy.runtime_task_stacks())
                .unwrap_err()
                .to_string();
            assert!(error.contains("overlaps"), "{error}");
        }
    }
}
