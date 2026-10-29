use super::*;
use crate::stack::tests::{Code, LOAD_RA, RET, SAVE_RA, SP_DOWN_16, SP_UP_16, TEXT, call, image};
use oer_image_stack::{Class, HartStack, LevelStack, TableEntry};

const BOOT: u32 = 3;
const OTHER: u32 = 7;
const SECONDARY: u32 = TEXT + 24;
const DEEPEST: u32 = TEXT + 48;
const SHORT: u32 = TEXT + 72;
const SECTION: u32 = TEXT + 96;

fn graph(secondary: u32, section: &'static str) -> Image {
    let function = |name, frame, transfer| Code {
        name,
        frame: Some(frame),
        words: vec![SP_DOWN_16, SAVE_RA, transfer, LOAD_RA, SP_UP_16, RET],
    };
    image(
        &[
            function("boot", 16, call(TEXT + 8, SECTION)),
            function("secondary", 16, secondary),
            function("deepest", 64, 0x13),
            function("short_handler", 16, call(SHORT + 8, SECTION)),
            Code {
                name: section,
                frame: Some(0),
                words: vec![RET],
            },
        ],
        &[],
    )
}

fn entries() -> BTreeMap<(u32, u32), String> {
    BTreeMap::from([
        ((BOOT, TEXT), "boot entry".into()),
        ((OTHER, SECONDARY), "secondary entry".into()),
    ])
}

#[test]
fn a_foreign_entry_cannot_acquire_or_release_the_section() {
    for section in [ENTRIES[0], ENTRIES[1]] {
        let image = graph(call(SECONDARY + 8, SECTION), section);
        let error = audit_roots(&image, &entries(), BOOT)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("secondary entry on hart 7 reaches"),
            "{error}"
        );
        assert!(error.contains(section), "{error}");
    }
}

#[test]
fn only_the_declared_hart_may_use_the_section() {
    let image = graph(0x13, ENTRIES[0]);
    let report = audit_roots(&image, &entries(), BOOT).unwrap();
    assert!(
        report.contains("secondary entry\tproven unreachable"),
        "{report}"
    );
    assert!(audit_roots(&image, &entries(), OTHER).is_err());
}

#[test]
fn a_foreign_hole_is_refused_even_without_a_linked_section() {
    let image = graph(0x0007_80e7, "unrelated"); // jalr ra, 0(a5)
    let error = audit_roots(&image, &entries(), BOOT)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("cannot prove secondary entry on hart 7"),
        "{error}"
    );
    assert!(error.contains("unresolved"), "{error}");
    // Current one-hart images keep their partial task bounds on the declared hart.
    let local = BTreeMap::from([((BOOT, SECONDARY), "local partial entry".into())]);
    audit_roots(&image, &local, BOOT).unwrap();
}

fn policy() -> StackPolicy {
    toml::from_str(&format!(
        r#"
schema = 6
max_move_bytes = 4096
coverage_policy = "coverage.toml"
[core.{BOOT}.task_stack]
root = "boot"
storage_symbol = "boot_stack"
minimum_free_bytes = 16
[core.{BOOT}.interrupt_stack]
[core.{OTHER}.task_stack]
root = "secondary"
storage_symbol = "secondary_stack"
minimum_free_bytes = 16
[core.{OTHER}.interrupt_stack]
"#
    ))
    .unwrap()
}

#[test]
fn every_handler_and_exception_is_a_root_including_the_shorter_handler() {
    let image = graph(0x13, ENTRIES[0]);
    let bound = image
        .with_bounds(|keep, control| {
            Ok(image
                .analysis
                .bound_with(DEEPEST, &image.resolutions, keep, control)?)
        })
        .unwrap()
        .0;
    let level = LevelStack {
        level: 1,
        bytes: bound.bytes,
        partial: bound.partial,
        frame: 0,
        bound,
    };
    let harts = vec![HartStack {
        core: OTHER,
        levels: vec![level.clone()],
        exception: level,
        exception_copies: 1,
        bytes: Some(128),
        partial: 128,
    }];
    let table = [DEEPEST, SHORT].map(|handler| TableEntry {
        source: handler,
        line: handler,
        level: 1,
        class: Class::Hardware,
        core: OTHER,
        handler: Some(handler),
    });
    let roots = roots(&image, &policy(), &table, &harts).unwrap();
    assert!(roots.contains_key(&(OTHER, SHORT)));
    assert!(roots[&(OTHER, DEEPEST)].contains("exception"));
    let error = audit_roots(&image, &roots, BOOT).unwrap_err().to_string();
    assert!(error.contains("hart7-P1-handler"), "{error}");
    assert!(error.contains(ENTRIES[0]), "{error}");
}

#[test]
fn a_task_poll_or_start_payload_reached_from_another_hart_is_refused() {
    // A resolved indirect entry into task/payload code uses the existing walk.
    let mut image = graph(0x0007_80e7, ENTRIES[0]);
    image
        .resolutions
        .add(SECONDARY + 8, oer_image_stack::Fact::FieldType, [SHORT]);
    assert!(audit_roots(&image, &entries(), BOOT).is_err());
}

#[test]
fn an_inlined_section_shim_is_detected_in_its_containing_function() {
    use oer_os_toolchain::{Tool, command, lld};
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("section.rs");
    let object = directory.path().join("section.o");
    let elf = directory.path().join("section.elf");
    std::fs::write(
        &source,
        r#"
#![no_std]
pub mod critical { pub mod hart {
    #[inline(always)]
    pub fn _critical_section_1_0_acquire(value: u32) -> u32 { value.wrapping_add(7) }
} }
#[inline(never)]
pub fn entry(value: u32) -> u32 { critical::hart::_critical_section_1_0_acquire(value) }
"#,
    )
    .unwrap();
    let output = command(Tool::Rustc)
        .unwrap()
        .args([
            "--edition=2024",
            "--crate-type=lib",
            "--crate-name=oer_espressif_staged_runtime",
            "--target=riscv32imafc-unknown-none-elf",
            "--emit=obj",
            "-Copt-level=3",
            "-Cdebuginfo=2",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(lld().unwrap())
        .args([
            "-m",
            "elf32lriscv",
            "--image-base=0",
            "--entry=0x1000",
            "-Ttext=0x1000",
        ])
        .arg(&object)
        .arg("-o")
        .arg(&elf)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = crate::stack::tests::from_bytes(std::fs::read(elf).unwrap());
    assert!(image.symbol(ENTRIES[0]).is_none());
    let owners = callers(&image);
    assert!(!owners.is_empty());
    let root = image.address_of("oer_espressif_staged_runtime::entry")[0];
    assert!(owners.contains(&root));
    let roots = BTreeMap::from([((OTHER, root), "foreign inlined section".into())]);
    let error = audit_roots(&image, &roots, BOOT).unwrap_err().to_string();
    assert!(error.contains("reaches"), "{error}");
}
