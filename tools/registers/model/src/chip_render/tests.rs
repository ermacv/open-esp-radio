//! The thin PAC renderer on a synthetic chip (`fixture/`): every field
//! class, register construct and ownership flag. Rendered code is compiled
//! against the workspace's `oer-portable-mmio` and `oer-portable-memory` and run on host memory; compile-fail
//! programs show what an owner cannot do.

use std::path::{Path, PathBuf};

use super::{names, render};
use crate::{RegisterEvidenceSet, chip_model::ChipModel, chip_policy::ChipPolicy};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/chip_render/fixture")
}

fn load(directory: &Path) -> (ChipModel, ChipPolicy) {
    let evidence = RegisterEvidenceSet::load_all(&[directory.join("evidence.toml")])
        .expect("the fixture evidence loads");
    let model = ChipModel::load(&directory.join("chip.toml"), &evidence)
        .unwrap_or_else(|error| panic!("the fixture root loads: {error}"));
    let policy = ChipPolicy::load(&directory.join("policy.toml"), &model, &evidence)
        .unwrap_or_else(|error| panic!("the fixture policy loads: {error}"));
    (model, policy)
}

/// The fixture's blocks, owners and layouts.
fn rendered() -> String {
    let (model, policy) = load(&fixture());
    render(&model, &policy).unwrap_or_else(|error| panic!("the fixture renders: {error}"))
}

/// The edited fixture's rendering.
fn rendered_with(edits: &[(&str, &str, &str)]) -> String {
    let directory = edited(edits);
    let (model, policy) = load(directory.path());
    render(&model, &policy).unwrap_or_else(|error| panic!("the edited fixture renders: {error}"))
}

/// A copy of the fixture in its own temporary directory, each edit
/// `(path, from, to)` applied to the file `path` (relative to the fixture).
fn edited(edits: &[(&str, &str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("a temporary directory");
    for relative in [
        "evidence.toml",
        "chip.toml",
        "policy.toml",
        "types/radio.toml",
        "types/etm.toml",
        "types/counter.toml",
        "layouts/rx-descriptor.toml",
    ] {
        let target = directory.path().join(relative);
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("the directory");
        let text = std::fs::read_to_string(fixture().join(relative)).expect("the fixture reads");
        let mut text = text;
        for (path, from, to) in edits.iter().filter(|(path, ..)| *path == relative) {
            assert!(text.contains(from), "{path} holds {from:?}");
            text = text.replacen(from, to, 1);
        }
        std::fs::write(target, text).expect("the copy is written");
    }
    // An edit of a file the fixture lacks, from nothing, writes it.
    for (path, _, text) in edits.iter().filter(|(_, from, _)| from.is_empty()) {
        std::fs::write(directory.path().join(path), text).expect("the new file is written");
    }
    directory
}

/// Builds `rendered` with the workspace's MMIO/memory crates and `main` into a
/// program in `directory` (the pinned cargo, offline, its own target
/// directory), runs it and returns its standard output.
fn run(rendered: &str, main: &str) -> String {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = cargo(directory.path(), rendered, main, "run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the program prints text")
}

/// The compiler's errors on a program over `rendered` that must not build.
fn refused(rendered: &str, main: &str) -> String {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = cargo(directory.path(), rendered, main, "build");
    assert!(!output.status.success(), "the program builds:\n{main}");
    String::from_utf8(output.stderr).expect("the compiler prints text")
}

fn cargo(directory: &Path, rendered: &str, main: &str, command: &str) -> std::process::Output {
    let mmio = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../portable/mmio");
    let memory = mmio.with_file_name("memory");
    std::fs::write(
        directory.join("Cargo.toml"),
        format!(
            "[package]\nname = \"rendered\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\noer-portable-mmio = {{ path = {:?} }}\n\
             oer-portable-memory = {{ path = {:?} }}\n\
             oer-portable-riscv = {{ path = {:?}, features = [\"host-hart\"] }}\n\
             bytemuck = {{ version = \"1.25.2\", default-features = false, features = [\"derive\", \"min_const_generics\"] }}\n\
             [workspace]\n",
            mmio.display().to_string(),
            memory.display().to_string(),
            mmio.with_file_name("riscv").display().to_string()
        ),
    )
    .expect("the program manifest is written");
    std::fs::create_dir(directory.join("src")).expect("the program source directory");
    std::fs::write(
        directory.join("src/main.rs"),
        format!("#![allow(dead_code, unused)]\nmod pac {{\n{rendered}\n}}\nuse pac::*;\n{main}"),
    )
    .expect("the program is written");
    std::process::Command::new(env!("CARGO"))
        .args([command, "--quiet", "--offline", "--manifest-path"])
        .arg(directory.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .current_dir(directory)
        .output()
        .expect("the pinned cargo runs")
}

/// The render error of the fixture with `edits`.
fn rejection(edits: &[(&str, &str, &str)]) -> super::Error {
    let directory = edited(edits);
    let (model, policy) = load(directory.path());
    match render(&model, &policy) {
        Ok(_) => panic!("the edited fixture renders"),
        Err(error) => error,
    }
}

#[test]
fn the_fixture_renders_deterministically_without_unsafe() {
    let (model, policy) = load(&fixture());
    let first = render(&model, &policy).expect("the fixture renders");
    let second = render(&model, &policy).expect("the fixture renders again");
    assert_eq!(first, second);
    for forbidden in ["unsafe {", "unsafe fn", "unsafe impl", "assume_owned("] {
        assert!(!first.contains(forbidden), "{forbidden}\n{first}");
    }
    assert!(first.contains("pub mod layouts {"), "{first}");
    // One module per type: the two RADIO instances share it.
    assert_eq!(first.matches("pub mod radio {").count(), 1);
    assert_eq!(first.matches("pub mod config {").count(), 1);
}

#[test]
fn only_the_irreducible_policy_items_render() {
    let rust = rendered();
    for item in [
        "pub fn store_vendor_word(&mut self)",
        "pub fn initialize_vendor_words_word_0060_opaque(&mut self)",
        "pub fn mask_and_clear_events(&mut self)",
        "pub fn sample_radio_events(&self)",
        "pub fn acknowledge_radio_events(",
        "pub fn publish_low(&mut self, low: oer_portable_mmio::Bits<24>)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

/// A record that would render nothing is refused, by its name: a fixed
/// image of fields the word's accessors write, and a zero register write
/// of a word that is not read-observed.
#[test]
fn a_policy_record_that_renders_nothing_is_refused() {
    let plain_image = edited(&[(
        "policy.toml",
        "[[fixed-register-sequences]]",
        "[[fixed-register-images]]\nname = \"set_slow_mode\"\nregister = \"RADIO0.RAD_ENABLE\"\nvalue = 0x00000001\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-sequences]]",
    )]);
    let (model, policy) = load(plain_image.path());
    let error = render(&model, &policy).expect_err("a plain image renders nothing");
    assert!(error.to_string().contains("set_slow_mode"), "{error}");

    let plain_zero = edited(&[(
        "policy.toml",
        "[[fixed-register-sequences]]",
        "[[zero-register-writes]]\nname = \"clear_command\"\nregister = \"RADIO0.RAD_COMMAND\"\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-sequences]]",
    )]);
    let evidence = RegisterEvidenceSet::load_all(&[plain_zero.path().join("evidence.toml")])
        .expect("the fixture evidence loads");
    let model = ChipModel::load(&plain_zero.path().join("chip.toml"), &evidence)
        .expect("the fixture root loads");
    let error = ChipPolicy::load(&plain_zero.path().join("policy.toml"), &model, &evidence)
        .expect_err("a zero write of a plain word renders nothing");
    assert!(error.to_string().contains("clear_command"), "{error}");
}

#[test]
fn a_fixed_hardware_updated_configuration_preserves_undeclared_bits_and_pulses_its_trigger() {
    let rust = rendered_with(&[
        (
            "policy.toml",
            "chip = \"testchip\"",
            "chip = \"testchip\"\n\n[[fixed-register-images]]\nname = \"configure_hw\"\nregister = \"RADIO0.RAD_HW\"\nvalue = 5\nsources = [\"SRC_DRIVER\"]",
        ),
        (
            "types/radio.toml",
            "[[registers]]\nname = \"RAD_DESC\"",
            "[[registers.fields]]\nname = \"DIVIDER_RESET\"\nbits = \"2\"\nclass = \"write-trigger\"\n\n[[registers]]\nname = \"RAD_DESC\"",
        ),
    ]);
    let result = run(
        &rust,
        r#"
        fn main() {
        for fill in [0, u32::MAX] {
            let mut memory = [fill; 128];
            {
                let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
                task.configure_hw();
                let value = task.read_hw();
                assert!(value.cfg());
                assert!(!value.stat());
            }
            // The synthetic trigger is asserted even from a zero image;
            // hardware-updated STAT is replaced and other bits survive.
            assert_eq!(memory[radio::hw::OFFSET / 4], (fill & !2) | 5);
        }
        println!("configured");
        }
    "#,
    );
    assert_eq!(result.trim(), "configured");
}

#[test]
fn fixed_policy_images_are_const_inputs_and_drive_their_writes() {
    for image in [1, 2] {
        let rust = rendered_with(&[(
            "policy.toml",
            "value = 0x00000003\nmasks = \"RADIO0.RAD_ENABLE\"",
            &format!("value = {image}\nmasks = \"RADIO0.RAD_ENABLE\""),
        )]);
        let output = run(
            &rust,
            &format!(
                r#"
                const _: () = assert!(radio::MASK_AND_CLEAR_EVENTS_IMAGE == {image});
                fn main() {{
                    let mut memory = [u32::MAX; 0x40];
                    {{
                        let (_, mut irq) = radio::radio0_host(&mut memory).expect("host block");
                        irq.mask_and_clear_events();
                    }}
                    // The policy change updates both the masked lanes and the
                    // W1C payload, while unrelated enable bits survive.
                    assert_eq!(memory[0x14 / 4], u32::MAX & !{image});
                    assert_eq!(memory[0x10 / 4], radio::MASK_AND_CLEAR_EVENTS_IMAGE);
                    println!("matched");
                }}
                "#,
            ),
        );
        assert_eq!(output.trim(), "matched");
    }
}

#[test]
fn unsampled_w1c_discard_uses_its_owner_and_preserves_undeclared_bits() {
    let rust = rendered_with(&[(
        "policy.toml",
        "chip = \"testchip\"",
        "chip = \"testchip\"\n\n[[fixed-register-images]]\nname = \"discard_startup\"\nregister = \"RADIO0.RAD_STATUS\"\nvalue = 3\nconsumed-event = \"All pre-bind pending belongs to the discarded epoch.\"\nsources = [\"SRC_DRIVER\"]",
    )]);
    let result = run(
        &rust,
        r#"
        fn main() {
            for fill in [0, u32::MAX] {
                let mut memory = [fill; 128];
                {
                    let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
                    task.discard_startup();
                    let status = task.read_status();
                    assert!(status.done() && status.error());
                }
                let changed: Vec<_> = memory.into_iter().filter(|value| *value != fill).collect();
                if fill == 0 { assert_eq!(changed, [3]); }
                else { assert!(changed.is_empty(), "undeclared bits must survive"); }
            }
            println!("discarded");
        }
    "#,
    );
    assert_eq!(result.trim(), "discarded");
}

#[test]
fn names_lose_the_block_prefix() {
    let (model, _) = load(&fixture());
    let radio = model.types().get("RADIO").expect("the RADIO type");
    assert_eq!(names::block_prefix(radio), "RAD_");
    let etm = model.types().get("ETM").expect("the ETM type");
    assert_eq!(names::block_prefix(etm), "");
    assert_eq!(names::strip("RAD_CONFIG", "RAD_"), "CONFIG");
    // What would start with a digit or be empty keeps its prefix.
    assert_eq!(names::strip("RAD_32K", "RAD_"), "RAD_32K");
    assert_eq!(names::strip("RAD_", "RAD_"), "RAD_");
    assert_eq!(names::strip("WORD_0060_OPAQUE", "RAD_"), "WORD_0060_OPAQUE");
    let rust = rendered();
    for item in [
        "pub fn modify_config(&mut self, change: impl FnOnce(&mut config::W))",
        "pub fn set_enable(&mut self, value: bool)",
        "pub fn read_slot(&self, index: oer_portable_mmio::Index<4>) -> slot::R",
        "pub fn read_ch_conf(&self, index: oer_portable_mmio::Index<2>) -> ch_conf::R",
        "pub fn read_ch1_cnt(&self) -> ch_cnt::R",
        "pub fn modify_word_0060_opaque(",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

const READ_OBSERVED: &[(&str, &str, &str)] = &[
    (
        "types/radio.toml",
        "class = \"unknown\"",
        "class = \"read-observed\"",
    ),
    (
        "policy.toml",
        "chip = \"testchip\"",
        "chip = \"testchip\"\n\n[[zero-register-writes]]\nname = \"publish_observed_zero\"\nregister = \"RADIO0.RAD_VAGUE\"\nsources = [\"SRC_DRIVER\"]",
    ),
];

#[test]
fn observed_reads_and_exact_zero_publication_have_no_generic_writer() {
    for sibling in [
        "",
        "\n\n[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"",
    ] {
        let observed = format!("class = \"read-observed\"{sibling}");
        let edits = [
            ("types/radio.toml", "class = \"unknown\"", observed.as_str()),
            READ_OBSERVED[1],
        ];
        let rust = rendered_with(&edits);
        let result = run(
            &rust,
            r#"
        fn main() {
            for fill in [0, 0x55555555, u32::MAX] {
                let mut memory = [fill; 128];
                {
                    let (mut task, _) = radio::radio0_host(&mut memory).expect("host");
                    assert_eq!(task.read_vague().vague().get(), fill & 15);
                    task.publish_observed_zero();
                    assert_eq!(task.read_vague().vague().get(), 0);
                }
                for (index, word) in memory.into_iter().enumerate() {
                    assert_eq!(word, if index == radio::vague::OFFSET / 4 { 0 } else { fill });
                }
            }
            println!("observed");
        }
        "#,
        );
        assert_eq!(result.trim(), "observed");
        for forbidden in [
            "task.modify_vague(|_| {});",
            "task.write_vague(|_| {});",
            "let _: Option<radio::vague::W> = None;",
        ] {
            let errors = refused(
                &rust,
                &format!(
                    "fn main() {{ let mut memory = [0; 128]; let (mut task, _) = radio::radio0_host(&mut memory).expect(\"host\"); {forbidden} }}"
                ),
            );
            assert!(
                errors.contains("E0599") || errors.contains("E0425"),
                "{errors}"
            );
        }
    }
}

#[test]
fn observed_words_reject_other_policy_writes_even_in_mixed_words() {
    for sibling in [
        "",
        "\n\n[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"",
    ] {
        for publication in [
            "[[fixed-register-images]]\nname = \"other_write\"\nregister = \"RADIO0.RAD_VAGUE\"\nvalue = 0\nwhole-word = true\nsources = [\"SRC_DRIVER\"]",
            "[[fixed-register-images]]\nname = \"other_write\"\nregister = \"RADIO0.RAD_VAGUE\"\nvalue = 1\nwhole-word = true\nsources = [\"SRC_DRIVER\"]",
            "[[fixed-register-sequences]]\nname = \"other_write\"\nsteps = [{ register = \"RADIO0.RAD_VAGUE\", value = 0 }]\nsources = [\"SRC_DRIVER\"]",
            "[[field-clear-modifies]]\nname = \"other_write\"\nfield = \"RADIO0.RAD_VAGUE.VAGUE\"\nsources = [\"SRC_DRIVER\"]",
            "[[field-clear-modifies]]\nname = \"other_write\"\nfield = \"RADIO0.RAD_VAGUE.CFG\"\nsources = [\"SRC_DRIVER\"]",
            "[[zero-based-field-writes]]\nname = \"other_write\"\nfields = [\"RADIO0.RAD_VAGUE.CFG\"]\nsources = [\"SRC_DRIVER\"]",
        ] {
            let observed = format!("class = \"read-observed\"{sibling}");
            let policy = format!("chip = \"testchip\"\n\n{publication}");
            let directory = edited(&[
                ("types/radio.toml", "class = \"unknown\"", &observed),
                ("policy.toml", "chip = \"testchip\"", &policy),
            ]);
            let evidence = RegisterEvidenceSet::load_all(&[directory.path().join("evidence.toml")])
                .expect("evidence");
            let model =
                ChipModel::load(&directory.path().join("chip.toml"), &evidence).expect("model");
            assert!(
                ChipPolicy::load(&directory.path().join("policy.toml"), &model, &evidence).is_err(),
                "{publication} beside {sibling} was admitted"
            );
        }
    }
}

#[test]
fn a_template_beside_its_single_register_keeps_its_index_place() {
    let directory = edited(&[
        (
            "types/radio.toml",
            "[[registers]]\nname = \"RAD_MIXED\"",
            "[[registers]]\nname = \"RAD_SLOT\"\noffset = 0x30\n\n[[registers.fields]]\nname = \"VALUE\"\nbits = \"0\"\nclass = \"plain\"\n\n[[registers]]\nname = \"RAD_MIXED\"",
        ),
        (
            "policy.toml",
            "\"RAD_SLOT%s\", \"RAD_MIXED\"",
            "\"RAD_SLOT%s\", \"RAD_SLOT\", \"RAD_MIXED\"",
        ),
        (
            "policy.toml",
            "\"RAD_PORT\", \"RAD_SLOT%s\", \"RAD_MIXED\"",
            "\"RAD_PORT\", \"RAD_SLOT%s\", \"RAD_SLOT\", \"RAD_MIXED\"",
        ),
    ]);
    let (model, policy) = load(directory.path());
    let rust = render(&model, &policy).expect("the edited fixture renders");
    assert!(
        rust.contains("pub fn read_slot(&self) -> slot::R"),
        "{rust}"
    );
    assert!(
        rust.contains("pub fn read_slot_n(&self, index: oer_portable_mmio::Index<4>) -> slot_n::R"),
        "{rust}"
    );
}

/// The fixture with a second source of `RADIO0`, both served by its
/// handler's part through one bank.
const TWO_SOURCES: &[(&str, &str, &str)] = &[
    (
        "chip.toml",
        "interrupt-sources = { count = 3,",
        "interrupt-sources = { count = 4,",
    ),
    (
        "chip.toml",
        "[[interrupts]]\nname = \"RADIO1_INTR\"",
        "[[interrupts]]\nname = \"RADIO0_NRT_INTR\"\nsource = 3\ninstance = \"RADIO0\"\nsources = [\"SRC_HEADER\"]\n\n[[interrupts]]\nname = \"RADIO1_INTR\"",
    ),
    (
        "policy.toml",
        "interrupt = \"RADIO0_INTR\"",
        "interrupt = [\"RADIO0_INTR\", \"RADIO0_NRT_INTR\"]",
    ),
];

#[test]
fn a_part_of_several_sources_names_them_all_and_no_single_one() {
    let rendered = rendered_with(TWO_SOURCES);
    let output = run(
        &rendered,
        "fn main() { println!(\"{:?}\", <radio::Irq as oer_portable_mmio::InterruptSources>::SOURCES); }",
    );
    assert_eq!(output.trim(), "[0, 3]");
    let errors = refused(
        &rendered,
        "fn main() { let _ = <radio::Irq as oer_portable_mmio::InterruptSource>::SOURCE; }",
    );
    assert!(
        errors.contains("InterruptSource` is not implemented"),
        "{errors}"
    );
}

#[test]
fn a_part_reaching_a_narrow_word_is_refused() {
    let error = rejection(&[
        (
            "policy.toml",
            "\"RAD_STATE\", \"RAD_CH%s.CONF\"",
            "\"RAD_STATE\", \"RAD_HALF%s\", \"RAD_CH%s.CONF\"",
        ),
        (
            "policy.toml",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
            "registers = [\"RAD_CH0.CNT\"]",
        ),
    ]);
    assert_eq!(error.entity, "RadioPeripherals.Task");
    assert!(error.reason.contains("16-bit word"), "{error}");
}

/// A part that owns an array of byte registers reaches each by its own byte
/// access, lists them apart from its words, and the claims' check counts
/// the word that holds them.
#[test]
fn a_part_owning_byte_registers_reaches_them_by_bytes() {
    let output = rendered_with(&[
        (
            "types/radio.toml",
            "width = 16\ndim = { count = 2, stride = 2 }",
            "width = 8\ndim = { count = 2, stride = 1 }",
        ),
        (
            "types/radio.toml",
            "name = \"V\"\nbits = \"15:0\"",
            "name = \"V\"\nbits = \"7:0\"",
        ),
        (
            "policy.toml",
            "\"RAD_STATE\", \"RAD_CH%s.CONF\"",
            "\"RAD_STATE\", \"RAD_HALF%s\", \"RAD_CH%s.CONF\"",
        ),
        (
            "policy.toml",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
            "registers = [\"RAD_CH0.CNT\"]",
        ),
    ]);
    assert!(
        output.contains("read_byte_indexed::<0x70, 0x1, 2>(index)"),
        "{output}"
    );
    assert!(output.contains("write_byte_indexed::<0x70, 0x1, 2>(index,"));
    assert!(output.contains("const OWNED_BYTES: &'static [usize] = &[0x70, 0x71];"));
    assert!(output.contains("const BYTES: &'static [usize] = &[0x70, 0x71];"));
    assert!(
        output.contains("<super::radio::TaskWords as oer_portable_mmio::Registers>::BYTES)")
            || output
                .contains("<super::radio::Radio0Division as oer_portable_mmio::Registers>::BYTES)"),
        "the claims' check lists the block's bytes: {output}"
    );
}

#[test]
fn a_division_of_more_than_four_parts_is_refused() {
    // Four watchers, each holding a word of its own and observing the
    // configuration, join the task and the handler in one division.
    let parts = ["RAD_CH0.CNT", "RAD_CH1.CNT", "RAD_SLOT0", "RAD_SLOT1"]
        .iter()
        .enumerate()
        .map(|(index, word)| {
            format!(
                "[[partitions.parts]]\nname = \"Watcher{index}\"\ninstance = \"RADIO0\"\nregisters = [\"{word}\"]\nobserves = [\"RAD_CONFIG\"]\ndescription = \"A watcher.\"\nsources = [\"SRC_DRIVER\"]\n\n"
            )
        })
        .collect::<String>();
    let error = rejection(&[
        (
            "policy.toml",
            "[[partitions]]\nname = \"SecondPeripherals\"",
            &format!("{parts}[[partitions]]\nname = \"SecondPeripherals\""),
        ),
        (
            "policy.toml",
            "\"RAD_SLOT%s\", \"RAD_MIXED\"",
            "\"RAD_SLOT2\", \"RAD_SLOT3\", \"RAD_MIXED\"",
        ),
        (
            "policy.toml",
            "\"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]",
            "\"RAD_CH%s.CONF\"]",
        ),
        (
            "policy.toml",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
            "registers = [\"RAD_HALF%s\"]",
        ),
    ]);
    assert_eq!(error.entity, "RADIO0");
    assert!(error.reason.contains("at most four"), "{error}");
}

/// Every accessor of the fixture's owners on host memory.
const BEHAVIOUR: &str = r#"
fn hex(word: u32) -> String {
    format!("{word:08x}")
}

fn main() {
    // RADIO0: the task and the interrupt handler divide one block.
    let mut memory = [0_u32; 0x40];
    memory[0x00 / 4] = 0xab08_0000 | 0x0001_0000 | 0x0002_0000 | 0x0000_0200;
    memory[0x08 / 4] = 0b11;
    memory[0x0c / 4] = 0b11;
    memory[0x14 / 4] = 0x0000_0103;
    memory[0x18 / 4] = 0x0000_0004;
    memory[0x1c / 4] = 0x0000_01ff;
    memory[0x48 / 4] = 0xffff_0000;
    memory[0x8c / 4] = 0x0000_0042;
    {
        let (mut task, mut irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        let config = task.read_config();
        println!("config version={} mode={:?}", config.version(), config.mode_variant());
        // The modify keeps read-only and reserved bits, zeroes the trigger
        // and the zero-only field.
        task.modify_config(|w| {
            w.set_enable(true)
                .set_level(radio::config::LevelRange::new(3).expect("3 is writable"));
        });
        println!("after modify {}", hex(task.read_config().level().get() | (u32::from(task.read_config().enable()) << 8)));
        task.trigger_config_start();
        task.write_command(|w| {
            w.set_opcode(radio::command::Opcode::Run).set_argument(7);
        });
        let status = task.sample_status();
        println!("status done={} error={}", status.done(), status.error());
        task.acknowledge_status(status.retain_error(false));
        let busy = task.start_cmd_go(true).expect_err("BUSY reads one");
        println!("busy running={:x} go_busy={}", busy.running, task.cmd_go_busy());
        println!("pop {:x}", task.pop_fifo());
        task.push_fifo(0x12);
        task.write_port(0xdead_beef);
        task.modify_slot(oer_portable_mmio::Index::constant::<2>(), |w| {
            w.set_value(0x1234);
        });
        task.modify_ch_conf(oer_portable_mmio::Index::new(1).expect("index 1"), |w| {
            w.set_en(true);
        });
        println!("ch1 cnt {:x}", task.read_ch1_cnt().n());
        task.store_vendor_word();
        task.publish_low(oer_portable_mmio::Bits::new(0x00ab_cdef).expect("24 bits"));
        println!("mixed on={}", task.read_mixed().on());
        println!("observed version {}", irq.read_config().version());
        let events = irq.sample_radio_events();
        println!("events rx={} tx={}", events.rx(), events.tx());
        irq.acknowledge_radio_events(events.retain_tx(false), Some(radio::RadioEventsConsumed::Aux));
        irq.mask_and_clear_events();
    }
    for (offset, what) in [
        (0x00, "config"),
        (0x04, "command"),
        (0x08, "status"),
        (0x10, "event clear"),
        (0x14, "enable"),
        (0x1c, "fifo"),
        (0x20, "port"),
        (0x48, "slot 2"),
        (0x88, "ch1 conf"),
        (0x60, "vendor word"),
        (0x64, "publish"),
    ] {
        println!("{what} {}", hex(memory[offset / 4]));
    }
    memory[0x18 / 4] = 0x0000_0050;
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.start_cmd_go(true).expect("nothing runs");
        println!("cmd busy={}", task.cmd_go_busy());
    }
    println!("cmd {}", hex(memory[0x18 / 4]));
    println!("source {}", <radio::Irq as oer_portable_mmio::InterruptSource>::SOURCE);

    // ETM: two parts split one trigger word by field.
    let mut memory = [0_u32; 0x10];
    {
        let (mut low, mut high) = etm::etm_host(&mut memory).expect("memory holds the block");
        low.trigger_channel_enable_set_ch1();
        println!("set after low {}", hex(memory_word(&low)));
        high.trigger_channel_enable_set_ch2();
        low.modify_ch1_event(|w| {
            w.set_id(9);
        });
        high.modify_ch2_event(|w| {
            w.set_id(7);
        });
        println!("high observes {}", high.read_channel_enable().ch2());
    }
    println!("etm set {} ch1 {} ch2 {}", hex(memory[1]), hex(memory[5]), hex(memory[6]));

    // The shared counter: every context strobes and stores through `&self`.
    let mut memory = [0_u32; 4];
    {
        let counter = counter::Counter::host(&mut memory).expect("memory holds the block");
        let shared = &counter;
        shared.trigger_update_update();
        shared.store_latch(5);
    }
    println!("counter {} {}", hex(memory[1]), hex(memory[3]));

    // A read-only copy owner.
    let mut memory = [0_u32; 8];
    memory[0x0c / 4] = 0b10;
    {
        let events = radio::SecondEvents::host(&mut memory).expect("memory holds the block");
        let copy = events;
        println!("copies {} {}", events.read_events().tx(), copy.read_events().rx());
    }

    println!(
        "interrupts {:?} {} {:x}",
        Interrupt::from_source(2),
        instances::radio1::RADIO1_INTR.source(),
        instances::radio1::BASE
    );
}

fn memory_word(_low: &etm::EtmLow<oer_portable_mmio::Host<'_>>) -> u32 {
    0
}
"#;

#[test]
fn every_owner_reaches_its_words_with_the_accessors_of_their_classes() {
    let output = run(&rendered(), BEHAVIOUR);
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "config version=171 mode=Some(Turbo)",
            "after modify 00000103",
            "status done=true error=true",
            "busy running=4 go_busy=false",
            "pop ff",
            "ch1 cnt 42",
            "mixed on=false",
            "observed version 171",
            "events rx=true tx=true",
            // Read-only and reserved bits kept, the trigger set alone, the
            // zero-only field zero, the mode kept.
            "config ab090231",
            // The reserved bits at their reset value, the fields set.
            "command 01000701",
            // Only the sampled bit kept is acknowledged.
            "status 00000001",
            // The mask image's value, written after the acknowledgement.
            "event clear 00000003",
            // The acknowledged sources masked, the other enable kept.
            "enable 00000100",
            "fifo 00000012",
            "port deadbeef",
            "slot 2 ffff1234",
            "ch1 conf 00000001",
            "vendor word 12345678",
            "publish ffabcdef",
            "cmd busy=true",
            // CFG kept, GO and FAST set.
            "cmd 00000053",
            "source 0",
            "set after low 00000000",
            "high observes false",
            // The last strobe's own bit alone.
            "etm set 00000004 ch1 00000009 ch2 00000007",
            "counter 00000001 00000005",
            "copies true false",
            "interrupts Some(SPARE) 1 60001000",
        ]
    );
}

/// Each case must fail to compile with its message: what an owner cannot
/// do. One program holds every case in its own function, so the compiler
/// reports each.
const FORBIDDEN: &[(&str, &str)] = &[
    // The handler only observes the configuration.
    (
        "fn observed(irq: &mut radio::Irq<oer_portable_mmio::Host<'_>>) { irq.modify_config(|_| {}); }",
        "no method named `modify_config`",
    ),
    // The clear word of a snapshot is written only by its acknowledgement.
    (
        "fn clear(irq: &mut radio::Irq<oer_portable_mmio::Host<'_>>) { irq.trigger_event_clr_rx(); }",
        "no method named `trigger_event_clr_rx`",
    ),
    // A write-only word has no read.
    (
        "fn command(task: &radio::Task<oer_portable_mmio::Host<'_>>) { task.read_command(); }",
        "no method named `read_command`",
    ),
    // A readable word with a write-only lane has no generic writer.
    (
        "fn mixed(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_mixed(|_| {}); }",
        "no method named `modify_mixed`",
    ),
    // A word the hardware also writes is only read.
    (
        "fn hardware(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_hw(|_| {}); }",
        "no method named `modify_hw`",
    ),
    // An unknown field has no accessor.
    (
        "fn vague(task: &radio::Task<oer_portable_mmio::Host<'_>>) { task.read_vague(); }",
        "no method named `read_vague`",
    ),
    // A word a read changes has none either.
    (
        "fn state(task: &radio::Task<oer_portable_mmio::Host<'_>>) { task.read_state(); }",
        "no method named `read_state`",
    ),
    // An address field takes its typed address, never an integer.
    (
        "fn pointer(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_desc(|w| { w.set_ptr(0x4080_0000); }); }",
        "expected `DmaRef<RxDescriptor>`, found integer",
    ),
    // A field written only with its values takes no integer.
    (
        "fn values(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_config(|w| { w.set_mode(1); }); }",
        "expected `Mode`, found integer",
    ),
    // A bounded field takes its bounded type.
    (
        "fn bounded(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_config(|w| { w.set_level(3); }); }",
        "expected `LevelRange`, found integer",
    ),
    // An unclaimed word has no owner.
    (
        "fn unclaimed(task: &radio::Task<oer_portable_mmio::Host<'_>>) { task.read_ch0_cnt(); }",
        "no method named `read_ch0_cnt`",
    ),
    // A part strobes only its own trigger fields.
    (
        "fn foreign(low: &mut etm::EtmLow<oer_portable_mmio::Host<'_>>) { low.trigger_channel_enable_set_ch2(); }",
        "no method named `trigger_channel_enable_set_ch2`",
    ),
    // ... and holds only its own elements.
    (
        "fn element(high: &mut etm::EtmHigh<oer_portable_mmio::Host<'_>>) { high.modify_ch0_event(|_| {}); }",
        "no method named `modify_ch0_event`",
    ),
    // A shared word is stored whole, never read and written back.
    (
        "fn shared(counter: &mut counter::Counter<oer_portable_mmio::Host<'_>>) { counter.modify_latch(|_| {}); }",
        "no method named `modify_latch`",
    ),
    // A snapshot is made only by its sample.
    (
        "fn forged() -> radio::RadioEventsSnapshot { radio::RadioEventsSnapshot(3) }",
        "is private",
    ),
];

#[test]
fn owners_refuse_what_their_parts_and_field_classes_forbid() {
    let program = FORBIDDEN
        .iter()
        .map(|(case, _)| *case)
        .collect::<Vec<_>>()
        .join("\n")
        + "\nfn main() {}\n";
    let stderr = refused(&rendered(), &program);
    for (case, message) in FORBIDDEN {
        assert!(stderr.contains(message), "{case}: {message}\n{stderr}");
    }
}

/// The fixture with the shapes the review of the first renderer found
/// mishandled and the hardware facts of the second: a command word with a
/// write-one-to-clear lane, a reserved bit beside write-one-to-clear
/// fields, a whole-word image behind a masks proof, a write-only field no
/// write may leave zero, a pointer's high field, a doorbell, a FIFO in a
/// repeated group, a shared array and a wide shared trigger.
const SHAPES: &[(&str, &str, &str)] = &[
    (
        "types/radio.toml",
        "[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"",
        "[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"\n\n[[registers.fields]]\nname = \"EVENT\"\nbits = \"8\"\nclass = \"write-one-to-clear\"",
    ),
    (
        "types/radio.toml",
        "name = \"RAD_STATUS\"\noffset = 0x08\n",
        "name = \"RAD_STATUS\"\noffset = 0x08\nreserved = { mask = 0x80000000, sources = [\"SRC_HEADER\"] }\n",
    ),
    (
        "types/radio.toml",
        "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"",
        "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"\nwrite-range = { min = 1, max = 255 }",
    ),
    (
        "types/radio.toml",
        "[[registers]]\nname = \"RAD_HALF%s\"",
        "[[registers]]\nname = \"RAD_LOW\"\noffset = 0x68\n\n[[registers.fields]]\nname = \"ADDRESS\"\nbits = \"15:0\"\nclass = \"plain\"\npointer = { points-to = \"RX_DESCRIPTOR\", placement = \"dma-window\", encoding = { width = 16, shift = 0, low-bits = true, high-field = \"RAD_HIGH.ADDRESS_HIGH\" }, sources = [\"SRC_DRIVER\"] }\n\n[[registers]]\nname = \"RAD_HIGH\"\noffset = 0x6c\n\n[[registers.fields]]\nname = \"ADDRESS_HIGH\"\nbits = \"31:16\"\nclass = \"plain\"\n\n[[registers]]\nname = \"RAD_RXCTL\"\noffset = 0x78\n\n[[registers.fields]]\nname = \"APPEND\"\nbits = \"0\"\nclass = \"self-clearing\"\n\n[[registers.fields]]\nname = \"WALK\"\nbits = \"31\"\nclass = \"plain\"\n\n[[registers]]\nname = \"RAD_HALF%s\"",
    ),
    (
        "types/radio.toml",
        "name = \"CNT\"\noffset = 0x4\n",
        "name = \"CNT\"\noffset = 0x4\nfifo = { sources = [\"SRC_HEADER\"] }\n",
    ),
    (
        "types/radio.toml",
        "[[commands]]",
        "[[doorbells]]\nregister = \"RAD_RXCTL\"\nfield = \"APPEND\"\nsources = [\"SRC_DRIVER\"]\n\n[[commands]]",
    ),
    (
        "types/counter.toml",
        "name = \"UPDATE\"\nbits = \"0\"",
        "name = \"UPDATE\"\nbits = \"3:0\"",
    ),
    (
        "types/counter.toml",
        "name = \"LATCH\"\noffset = 0x0c\n",
        "name = \"LATCH%s\"\noffset = 0x0c\ndim = { count = 2, stride = 4 }\n",
    ),
    (
        "policy.toml",
        "\"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]",
        "\"RAD_CH%s.CONF\", \"RAD_CH%s.CNT\", \"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\"]",
    ),
    (
        "policy.toml",
        "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
        "registers = [\"RAD_HALF%s\"]",
    ),
    (
        "policy.toml",
        "\"RAD_HALF%s\", \"RAD_STATE\", \"RAD_CH%s.CONF\", \"RAD_CH%s.CNT\"]",
        "\"RAD_HALF%s\", \"RAD_STATE\", \"RAD_CH%s.CONF\", \"RAD_CH%s.CNT\", \"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\"]",
    ),
    (
        "policy.toml",
        "registers = [\"COUNT\", \"UPDATE\", \"LATCH\"]",
        "registers = [\"COUNT\", \"UPDATE\", \"LATCH%s\"]",
    ),
    (
        "policy.toml",
        "masks = \"RADIO0.RAD_ENABLE\"\n",
        "masks = \"RADIO0.RAD_ENABLE\"\nwhole-word = true\n",
    ),
];

/// The shapes' accessors on host memory.
const SHAPES_BEHAVIOUR: &str = r#"
fn hex(word: u32) -> String {
    format!("{word:08x}")
}

/// A typed address reaches the pointer's setter; never called, as only
/// the hardware's placement makes one.
fn publish(task: &mut radio::Task<oer_portable_mmio::Host<'_>>, next: oer_portable_memory::DmaRef<layouts::RxDescriptor>) {
    task.modify_low(|w| {
        w.set_address(next);
    });
}

fn main() {
    let mut memory = [0_u32; 0x40];
    memory[0x08 / 4] = 0x8000_0003;
    memory[0x14 / 4] = 0x0000_0103;
    memory[0x18 / 4] = 0x0000_0100;
    memory[0x6c / 4] = 0x0000_1234;
    memory[0x78 / 4] = 0x0000_0001;
    memory[0x84 / 4] = 0x0000_0011;
    memory[0x8c / 4] = 0x0000_0022;
    {
        let (mut task, mut irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        // The command start zeroes the write-one-to-clear lane it read.
        task.start_cmd_go(false).expect("nothing runs");
        // The acknowledgement keeps the reserved bit it reads now.
        let status = task.sample_status();
        task.acknowledge_status(status.retain_error(false));
        // A write from zero takes the argument no write may leave zero.
        task.write_command(radio::command::ArgumentRange::new(7).expect("7 is writable"), |w| {
            w.set_opcode(radio::command::Opcode::Run);
        });
        // The high field takes its placement's prefix alone.
        task.modify_high(|w| {
            w.set_address_high_prefix();
        });
        // The doorbell: pending, written back as read, rung alone.
        println!("pending {}", task.rxctl_append_pending());
        task.modify_rxctl(|w| {
            w.set_walk(true);
        });
        println!("rxctl {}", hex(task.read_rxctl().append() as u32 | ((task.read_rxctl().walk() as u32) << 31)));
        // The readable word with a write-only lane, written from zero.
        task.write_mixed(|w| {
            w.set_on(true).set_key(0x5a);
        });
        // The FIFO of a repeated group, by index.
        println!("fifo {:x} {:x}", task.pop_ch_cnt(oer_portable_mmio::Index::constant::<0>()), task.pop_ch_cnt(oer_portable_mmio::Index::constant::<1>()));
        // The whole-word image masks its sources first.
        irq.mask_and_clear_events();
    }
    for (offset, what) in [
        (0x18, "cmd"),
        (0x08, "status"),
        (0x04, "command"),
        (0x6c, "high"),
        (0x50, "mixed"),
        (0x14, "enable"),
        (0x10, "event clear"),
    ] {
        println!("{what} {}", hex(memory[offset / 4]));
    }
    memory[0x78 / 4] = 0;
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.trigger_rxctl_append();
    }
    println!("rung {}", hex(memory[0x78 / 4]));

    // The shared array's elements and the wide trigger, through `&self`.
    let mut memory = [0_u32; 6];
    {
        let counter = counter::Counter::host(&mut memory).expect("memory holds the block");
        let shared = &counter;
        shared.trigger_update_update(oer_portable_mmio::Bits::new(9).expect("4 bits"));
        shared.store_latch1(5);
    }
    println!("counter {} {}", hex(memory[1]), hex(memory[4]));
}
"#;

#[test]
fn the_reviewed_shapes_keep_their_lanes_and_constraints() {
    let output = run(&rendered_with(SHAPES), SHAPES_BEHAVIOUR);
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        [
            "pending true",
            // The pending doorbell written back as read beside WALK.
            "rxctl 80000001",
            "fifo 11 22",
            // GO set; the write-one-to-clear EVENT lane zero.
            "cmd 00000001",
            // The reserved bit kept, the sampled DONE acknowledged.
            "status 80000001",
            // The reserved reset, ARGUMENT 7, RUN.
            "command 01000701",
            // The dma-window's prefix 0x4080 in bits 31:16; the bits of no
            // field kept.
            "high 40801234",
            "mixed 00005a01",
            // The masked sources cleared, the other enable kept.
            "enable 00000100",
            "event clear 00000003",
            "rung 00000001",
            "counter 00000009 00000005",
        ]
    );
}

#[test]
fn the_reviewed_shapes_refuse_unadmitted_values() {
    let program = [
        // A data port's address takes its typed address.
        "fn port(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.write_desc(0xdead_beef); }",
        // ARGUMENT is never written zero: the write takes it.
        "fn omitted(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.write_command(|w: &mut radio::command::W| { w.set_opcode(radio::command::Opcode::Run); }); }",
        // The high field takes no integer.
        "fn high(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_high(|w| { w.set_address_high(0x1234); }); }",
        // The doorbell is rung only by its trigger.
        "fn ring(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.modify_rxctl(|w| { w.set_append(true); }); }",
        // The step's write range wins over its complete enumeration.
        "fn step(mode: &mut counter::Mode<oer_portable_mmio::Host<'_>>) { mode.modify_mode(|w| { w.set_step(counter::mode::Step::Off); }); }",
        // A zero-only trigger has no trigger.
        "fn quiet(task: &mut radio::Task<oer_portable_mmio::Host<'_>>) { task.trigger_config_start(); }",
        "fn main() {}",
    ]
    .join("\n");
    let edits = SHAPES
        .iter()
        .copied()
        .chain([
            (
                "types/radio.toml",
                "name = \"RAD_DESC\"\noffset = 0x5c\n",
                "name = \"RAD_DESC\"\noffset = 0x5c\ndata-port = { sources = [\"SRC_HEADER\"] }\n",
            ),
            (
                "types/counter.toml",
                "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"",
                "name = \"STEP\"\nbits = \"0\"\nclass = \"plain\"\nvalues = [{ name = \"OFF\", value = 0 }, { name = \"ON\", value = 1 }]\nwrite-range = { min = 1, max = 1 }",
            ),
            (
                "types/radio.toml",
                "name = \"START\"\nbits = \"16\"\nclass = \"write-trigger\"",
                "name = \"START\"\nbits = \"16\"\nclass = \"write-trigger\"\nwrite-range = { min = 0, max = 0 }",
            ),
        ])
        .collect::<Vec<_>>();
    let stderr = refused(&rendered_with(&edits), &program);
    for message in [
        "expected `DmaRef<RxDescriptor>`, found integer",
        "this method takes 2 arguments but 1 argument was supplied",
        "no method named `set_address_high` found",
        "no method named `set_append` found",
        "expected `StepRange`, found `Step`",
        "no method named `trigger_config_start` found",
    ] {
        assert!(stderr.contains(message), "{message}\n{stderr}");
    }
}

#[test]
fn a_word_with_an_unknown_lane_has_no_writer_from_zero() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"",
        "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"\n\n[[registers.fields]]\nname = \"UNRESOLVED\"\nbits = \"23:16\"\nclass = \"unknown\"",
    )]);
    assert!(!rust.contains("pub fn write_command("), "{rust}");
}

/// The data port `RAD_PORT` with its one field `hardware-written`, the
/// command port's data word the hardware also fills; with `port` false the
/// same word is no data port.
fn hardware_written_port(port: bool) -> String {
    let mut edits = vec![(
        "types/radio.toml",
        "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"plain\"",
        "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"hardware-written\"",
    )];
    if !port {
        edits.push((
            "types/radio.toml",
            "name = \"RAD_PORT\"\noffset = 0x20\ndata-port = { sources = [\"SRC_HEADER\"] }\n",
            "name = \"RAD_PORT\"\noffset = 0x20\n",
        ));
    }
    rendered_with(&edits)
}

#[test]
fn a_data_port_stores_its_hardware_written_word_whole() {
    let rust = hardware_written_port(true);
    assert!(rust.contains("pub fn write_port("), "{rust}");
    let output = run(
        &rust,
        r#"fn main() {
    let mut memory = [0_u32; 64];
    memory[0x20 / 4] = 0x1111_1111;
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.write_port(0xdead_beef);
        println!("read {:08x}", task.read_port().value());
    }
    println!("port {:08x}", memory[0x20 / 4]);
}
"#,
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["read deadbeef", "port deadbeef"]
    );
}

#[test]
fn a_hardware_written_word_outside_a_data_port_has_no_writer() {
    let rust = hardware_written_port(false);
    assert!(rust.contains("pub fn read_port("), "{rust}");
    assert!(!rust.contains("pub fn write_port("), "{rust}");
}

#[test]
fn a_command_start_refuses_a_lane_it_can_neither_keep_nor_zero() {
    let error = rejection(&[(
        "types/radio.toml",
        "[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"",
        "[[registers.fields]]\nname = \"CFG\"\nbits = \"7:4\"\nclass = \"plain\"\n\n[[registers.fields]]\nname = \"SEEN\"\nbits = \"8\"\nclass = \"hardware-written\"",
    )]);
    assert_eq!(error.entity, "RAD_CMD.SEEN");
    assert!(error.reason.contains("neither keep nor zero"), "{error}");
}

#[test]
fn generated_names_never_collide() {
    // A field's getter against another's `_variant` getter.
    let error = rejection(&[(
        "types/radio.toml",
        "name = \"VERSION\"\nbits = \"31:24\"",
        "name = \"MODE_VARIANT\"\nbits = \"31:24\"",
    )]);
    assert_eq!(error.entity, "RADIO.RAD_CONFIG.MODE_VARIANT");
    assert!(error.reason.contains("mode_variant"), "{error}");
    // Two types whose modules normalize alike.
    let error = rejection(&[
        ("types/etm.toml", "name = \"ETM\"", "name = \"RADIO_\""),
        ("chip.toml", "type = \"ETM\"", "type = \"RADIO_\""),
    ]);
    assert!(error.reason.contains("module `radio`"), "{error}");
}

#[test]
fn a_snapshot_samples_its_named_events_and_documents_its_consumed_event() {
    let rust = rendered_with(&[
        (
            "policy.toml",
            "status = \"RADIO0.RAD_EVENTS\"\n",
            "status = \"RADIO0.RAD_EVENTS\"\nevents = [\"RADIO0.RAD_EVENTS.RX\"]\n",
        ),
        (
            "policy.toml",
            "masks = \"RADIO0.RAD_ENABLE\"\n",
            "consumed-event = \"The task read both events.\\nIt has finished using the associated data.\"\n",
        ),
    ]);
    assert!(
        rust.contains("pub const EVENTS: u32 = 0x00000001;"),
        "{rust}"
    );
    // Every line of the explanation stays documentation.
    let output = run(&rust, "fn main() {}");
    assert!(output.is_empty());
}

/// The shapes with a word of two write triggers (`RAD_KICK`) and the
/// descriptor pointer published by a field-or-modify instead of its pointer
/// operation.
fn with_trigger_word_and_pointer_or() -> std::vec::Vec<(&'static str, &'static str, &'static str)> {
    let mut edits = SHAPES.to_vec();
    edits.extend_from_slice(&[
        (
            "types/radio.toml",
            "[[groups]]\nname = \"RAD_CH%s\"",
            "[[registers]]\nname = \"RAD_KICK\"\noffset = 0x90\n\n[[registers.fields]]\nname = \"GO\"\nbits = \"0\"\nclass = \"write-trigger\"\n\n[[registers.fields]]\nname = \"LEVEL\"\nbits = \"6:4\"\nclass = \"write-trigger\"\n\n[[groups]]\nname = \"RAD_CH%s\"",
        ),
        (
            "policy.toml",
            "\"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\"]",
            "\"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\", \"RAD_KICK\"]",
        ),
        (
            "policy.toml",
            "\"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\"]",
            "\"RAD_LOW\", \"RAD_HIGH\", \"RAD_RXCTL\", \"RAD_KICK\"]",
        ),
        (
            "policy.toml",
            "[[fixed-register-sequences]]",
            "[[field-or-modifies]]\nname = \"or_descriptor\"\nfield = \"RADIO0.RAD_DESC.PTR\"\nsources = [\"SRC_DRIVER\"]\n\n[[fixed-register-sequences]]",
        ),
    ]);
    edits
}

const POINTER_AND_TRIGGERS_BEHAVIOUR: &str = r#"
fn hex(word: u32) -> String {
    format!("{word:08x}")
}

/// The vendor's OR takes a typed reference alone; never called, as only the
/// hardware's placement makes one.
fn or(task: &mut radio::Task<oer_portable_mmio::Host<'_>>, next: oer_portable_memory::DmaRef<layouts::RxDescriptor>) {
    task.or_descriptor(next);
}

fn main() {
    let mut memory = [0_u32; 0x40];
    memory[0x68 / 4] = 0xabcd_1234;
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        // The zero image of a pointer, every other bit kept.
        task.modify_low(|w| {
            w.clear_address();
        });
        // Two triggers in one write.
        task.trigger_kick(|t| {
            t.go().level(oer_portable_mmio::Bits::new(5).expect("3 bits"));
        });
    }
    println!("low {}", hex(memory[0x68 / 4]));
    println!("kick {}", hex(memory[0x90 / 4]));
    memory[0x90 / 4] = 0;
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        // A field's own trigger stays.
        task.trigger_kick_go();
    }
    println!("go {}", hex(memory[0x90 / 4]));
}
"#;

/// A whole-word step of a fixed sequence stores its image whole, and a
/// snapshot token reads back the bits it sampled.
const SEQUENCE_AND_SAMPLE: &str = r#"
fn main() {
    let mut memory = [0_u32; 0x40];
    memory[0x0c / 4] = 0b10;
    {
        let (mut task, mut irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.initialize_vendor_words_word_0060_opaque();
        let events = irq.sample_radio_events();
        println!("sampled {:x}", events.bits());
        irq.acknowledge_radio_events(events, None);
    }
    println!("vendor word {:08x}", memory[0x60 / 4]);
}
"#;

#[test]
fn a_pointer_takes_its_zero_image_and_triggers_share_one_write() {
    let output = run(
        &rendered_with(&with_trigger_word_and_pointer_or()),
        POINTER_AND_TRIGGERS_BEHAVIOUR,
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["low abcd0000", "kick 00000051", "go 00000001"]
    );
}

#[test]
fn a_pointer_or_renders_on_its_part_and_reads_the_word_fresh() {
    let rust = rendered_with(&with_trigger_word_and_pointer_or());
    for item in [
        "pub fn or_descriptor(&mut self, value: oer_portable_memory::DmaRef<super::layouts::RxDescriptor>)",
        "self.0.write::<0x5c>(self.0.read::<0x5c>() | value.address().get());",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    // A pointer setter never takes an integer, its zero image aside.
    let refusal = refused(
        &rust,
        "fn main() {\n    let mut memory = [0_u32; 0x40];\n    let (mut task, _irq) = radio::radio0_host(&mut memory).unwrap();\n    task.modify_low(|w| { w.set_address(0); });\n}\n",
    );
    assert!(refusal.contains("mismatched types"), "{refusal}");
}

#[test]
fn a_snapshot_clear_word_has_no_trigger_writer() {
    let rust = rendered();
    // `RAD_EVENT_CLR` holds three triggers but only its acknowledgement
    // writes it.
    assert!(!rust.contains("pub fn trigger_event_clr("), "{rust}");
}

#[test]
fn a_whole_word_sequence_step_is_one_method_and_a_snapshot_reads_its_bits() {
    let output = run(&rendered(), SEQUENCE_AND_SAMPLE);
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["sampled 2", "vendor word 0badf00d"]
    );
}

/// An element-range sequence step: one accessor named after its sequence
/// that stores the whole word to the selected elements in ascending order.
fn with_element_range() -> String {
    rendered_with(&[(
        "policy.toml",
        "[[fixed-register-sequences]]\nname = \"initialize_vendor_words\"",
        "[[fixed-register-sequences]]\nname = \"store_slots\"\nsteps = [{ register = \"RADIO0.RAD_SLOT%s\", first = 1, count = 2, value = 0xdeadbeef }]\nsources = [\"SRC_VENDOR\"]\n\n[[fixed-register-sequences]]\nname = \"initialize_vendor_words\"",
    )])
}

#[test]
fn an_element_range_is_one_accessor_over_its_elements_in_order() {
    let rust = with_element_range();
    assert_eq!(rust.matches("pub fn store_slots(").count(), 1, "{rust}");
    assert!(!rust.contains("pub fn store_slots_rad_slot"), "{rust}");
    let output = run(
        &rust,
        r#"
fn main() {
    let mut memory = [0x1111_1111_u32; 0x40];
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.store_slots();
    }
    println!("{:08x?}", &memory[0x40 / 4..0x50 / 4]);
}
"#,
    );
    // Elements 1 and 2 hold the whole image, undeclared bits included;
    // elements 0 and 3 are untouched.
    assert_eq!(output.trim(), "[11111111, deadbeef, deadbeef, 11111111]");
}

#[test]
fn an_element_range_over_a_part_s_share_of_an_array_writes_only_that_share() {
    // The task holds elements 0 and 1 of the four slots; no part holds 2
    // and 3. A range over the task's two elements builds and writes them.
    let rust = rendered_with(&[
        (
            "policy.toml",
            "\"RAD_PORT\", \"RAD_SLOT%s\", \"RAD_MIXED\", \"RAD_VAGUE\", \"RAD_HW\", \"RAD_DESC\", \"WORD_0060_OPAQUE\", \"RAD_PUBLISH\", \"RAD_STATE\", \"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]",
            "\"RAD_PORT\", \"RAD_MIXED\", \"RAD_VAGUE\", \"RAD_HW\", \"RAD_DESC\", \"WORD_0060_OPAQUE\", \"RAD_PUBLISH\", \"RAD_STATE\", \"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]\nelements = [{ register = \"RAD_SLOT%s\", first = 0, count = 2 }]",
        ),
        (
            "policy.toml",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]\nelements = [{ register = \"RAD_SLOT%s\", first = 2, count = 2 }]",
        ),
        (
            "policy.toml",
            "[[fixed-register-sequences]]\nname = \"initialize_vendor_words\"",
            "[[fixed-register-sequences]]\nname = \"store_slots\"\nsteps = [{ register = \"RADIO0.RAD_SLOT%s\", first = 0, count = 2, value = 0xdeadbeef }]\nsources = [\"SRC_VENDOR\"]\n\n[[fixed-register-sequences]]\nname = \"initialize_vendor_words\"",
        ),
    ]);
    let output = run(
        &rust,
        r#"
fn main() {
    let mut memory = [0x1111_1111_u32; 0x40];
    {
        let (mut task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
        task.store_slots();
    }
    println!("{:08x?}", &memory[0x40 / 4..0x50 / 4]);
}
"#,
    );
    assert_eq!(output.trim(), "[deadbeef, deadbeef, 11111111, 11111111]");
}

#[test]
fn a_partition_image_reads_each_part_s_readable_words_whole() {
    let rust = rendered_with(&[(
        "policy.toml",
        "chip = \"testchip\"\n",
        "chip = \"testchip\"\n\n[[partition-image-reads]]\nname = \"second_image\"\npartition = \"SecondPeripherals\"\nsources = [\"SRC_DRIVER\"]\n",
    )]);
    // Radio 0's partition reads no image.
    assert_eq!(
        rust.matches("pub const SECOND_IMAGE_OFFSETS").count(),
        2,
        "{rust}"
    );
    let output = run(
        &rust,
        r#"
fn main() {
    let mut config = [0_u32; 0x40];
    config[0] = 0xdead_beef;
    let mut events = [0_u32; 0x40];
    events[0x0c / 4] = 0x8000_0003;
    let second = radio::Second::host(&mut config).expect("memory holds the block");
    let view = radio::SecondEvents::host(&mut events).expect("memory holds the block");
    println!(
        "{:x?} {:x?}",
        radio::Second::<oer_portable_mmio::Host<'_>>::SECOND_IMAGE_OFFSETS,
        radio::SecondEvents::<oer_portable_mmio::Host<'_>>::SECOND_IMAGE_OFFSETS,
    );
    println!("{:08x} {:08x}", second.second_image(0).expect("word 0"), view.second_image(0).expect("word 0"));
    println!("{:?}", second.second_image(1));
}
"#,
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["[0] [c]", "deadbeef 80000003", "None"]
    );
}

/// The fixture with gate, reset, shared and mux links of `RADIO0` and
/// `RADIO1` into the counter's `MODE` word, which the `Mode` part holds.
fn linked() -> String {
    rendered_with(&[
        (
            "types/counter.toml",
            "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"\n",
            "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"\n\n\
             [[registers.fields]]\nname = \"RADIO_CLK_EN\"\nbits = \"4\"\nclass = \"plain\"\n\n\
             [[registers.fields]]\nname = \"RADIO_RST\"\nbits = \"5\"\nclass = \"plain\"\n\n\
             [[registers.fields]]\nname = \"COMMON_CLK_EN\"\nbits = \"6\"\nclass = \"plain\"\n",
        ),
        (
            "chip.toml",
            "name = \"RADIO0\"\ntype = \"RADIO\"\nbase = 0x60000000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n",
            "name = \"RADIO0\"\ntype = \"RADIO\"\nbase = 0x60000000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n\
             clocks = [\n  { role = \"bus\", field = \"COUNTER.MODE.RADIO_CLK_EN\", sources = [\"SRC_HEADER\"] },\n  \
             { role = \"common\", field = \"COUNTER.MODE.COMMON_CLK_EN\", sources = [\"SRC_HEADER\"] },\n]\n\
             resets = [\n  { role = \"block\", field = \"COUNTER.MODE.RADIO_RST\", sources = [\"SRC_HEADER\"] },\n]\n\
             muxes = [\n  { role = \"step\", field = \"COUNTER.MODE.STEP\", inputs = { 0 = \"XTAL\" }, sources = [\"SRC_HEADER\"] },\n]\n",
        ),
        (
            "chip.toml",
            "name = \"RADIO1\"\ntype = \"RADIO\"\nbase = 0x60001000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n",
            "name = \"RADIO1\"\ntype = \"RADIO\"\nbase = 0x60001000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n\
             clocks = [\n  { role = \"common\", field = \"COUNTER.MODE.COMMON_CLK_EN\", sources = [\"SRC_HEADER\"] },\n]\n",
        ),
        (
            "chip.toml",
            "[[ram-regions]]",
            "[[shared-controls]]\nname = \"RADIO_COMMON\"\nfield = \"COUNTER.MODE.COMMON_CLK_EN\"\n\
             instances = [\"RADIO0\", \"RADIO1\"]\nsources = [\"SRC_HEADER\"]\n\n\
             [[clock-sources]]\nname = \"XTAL\"\nsources = [\"SRC_HEADER\"]\n\n[[ram-regions]]",
        ),
    ])
}

#[test]
fn a_root_link_is_a_control_on_the_part_holding_its_word() {
    let rendered = linked();
    for method in [
        "pub fn set_radio0_bus_clock(&mut self, on: bool)",
        "pub fn set_radio0_block_reset(&mut self, asserted: bool)",
        "pub fn set_radio_common_clock(&mut self, on: bool)",
    ] {
        assert!(rendered.contains(method), "{method} is generated");
    }
    let stdout = run(
        &rendered,
        "fn main() {\n\
         let mut memory = [0_u32, 0, 0x5, 0];\n\
         {\n\
         let mut mode = counter::Mode::host(&mut memory).expect(\"memory holds the block\");\n\
         mode.set_radio0_bus_clock(true);\n\
         mode.set_radio_common_clock(true);\n\
         mode.set_radio0_block_reset(true);\n\
         mode.set_radio0_block_reset(false);\n\
         }\n\
         println!(\"{:#x}\", memory[2]);\n\
         }\n",
    );
    assert_eq!(
        stdout.trim(),
        "0x55",
        "the gates open, the reset pulses, STEP keeps 5"
    );
}

#[test]
fn a_shared_field_or_a_mux_has_no_control_named_after_one_user() {
    let rendered = linked();
    for method in [
        "fn set_radio0_common_clock",
        "fn set_radio1_common_clock",
        "fn set_radio0_step_",
    ] {
        assert!(!rendered.contains(method), "{method} is not generated");
    }
}

#[test]
fn a_link_into_an_unheld_word_has_no_control() {
    let rendered = rendered_with(&[
        (
            "types/counter.toml",
            "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"\n",
            "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"\n\n\
             [[registers.fields]]\nname = \"RADIO_CLK_EN\"\nbits = \"4\"\nclass = \"plain\"\n",
        ),
        (
            "chip.toml",
            "name = \"RADIO0\"\ntype = \"RADIO\"\nbase = 0x60000000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n",
            "name = \"RADIO0\"\ntype = \"RADIO\"\nbase = 0x60000000\nextent = { size = 0x1000 }\nsources = [\"SRC_HEADER\"]\n\
             clocks = [\n  { role = \"bus\", field = \"COUNTER.MODE.RADIO_CLK_EN\", sources = [\"SRC_HEADER\"] },\n]\n",
        ),
        (
            "policy.toml",
            "[[partitions.parts]]\nname = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]\n\n",
            "",
        ),
        (
            "policy.toml",
            "[[unclaimed]]\ninstance = \"RADIO0\"",
            "[[unclaimed]]\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]\nreason = \"No driver reaches it.\"\n\n[[unclaimed]]\ninstance = \"RADIO0\"",
        ),
    ]);
    assert!(!rendered.contains("fn set_radio0_bus_clock"));
}

#[test]
fn field_geometry_and_equal_encoding_names_are_generated_without_ambiguous_reads() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "{ name = \"FAST\", value = 1 }",
        "{ name = \"FAST\", value = 1 }, { name = \"FAST_WRITE\", value = 1 }",
    )]);
    assert!(
        !rust.contains("self.bits() << 0"),
        "zero-position enum values need no shift"
    );
    assert_eq!(
        run(
            &rust,
            r#"
fn main() {
    assert_eq!(radio::config::LEVEL_WIDTH, 3);
    assert_eq!(radio::config::LEVEL_MAX, 7);
    assert_eq!(radio::config::Mode::FastWrite.bits(), radio::config::Mode::Fast.bits());
    assert_eq!(radio::config::Mode::from_bits(1), Some(radio::config::Mode::Fast));
    assert_eq!(radio::config::Mode::FastWrite.register_bits(), 1 << 8);
    assert_eq!(radio::command::Opcode::Run.register_bits(), 1);
}
"#
        ),
        ""
    );
}
#[test]
fn sourced_block_geometry_is_rendered() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "schema = 3",
        "schema = 3\n[[geometry]]\nname = \"LINE_BYTES\"\nvalue = 64\nsources = [\"SRC_HEADER\"]\n",
    )]);
    assert_eq!(
        run(&rust, "fn main() { assert_eq!(radio::LINE_BYTES, 64); }"),
        ""
    );
}

#[test]
fn mixed_word_triggers_keep_rw_and_unassigned_bits_and_zero_wtc_and_other_wt() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "name = \"KEY\"\nbits = \"15:8\"\nclass = \"write-only\"",
        "name = \"EVENT\"\nbits = \"8\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"START\"\nbits = \"16\"\nclass = \"write-trigger\"\n\n[[registers.fields]]\nname = \"OTHER\"\nbits = \"17\"\nclass = \"write-trigger\"",
    )]);
    assert!(!rust.contains("pub fn modify_mixed("));
    assert!(!rust.contains("pub fn write_mixed("));
    assert!(!rust.contains("unsafe {"));
    let output = run(
        &rust,
        r#"
fn main() {
    for initial in [0, u32::MAX, 0x8102_0101, 0x0003_0100] {
        let mut memory = [0_u32; 0x40];
        memory[0x50 / 4] = initial;
        {
            let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
            task.trigger_mixed_start();
        }
        assert_eq!(memory[0x50 / 4], (initial & !0x0003_0100) | 0x0001_0000);
        {
            let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
            task.trigger_mixed_other();
        }
        assert_eq!(memory[0x50 / 4], (initial & !0x0003_0100) | 0x0002_0000);
    }
    println!("mixed triggers preserve RW and never acknowledge WTC");
}
"#,
    );
    assert_eq!(
        output.trim(),
        "mixed triggers preserve RW and never acknowledge WTC"
    );
}

#[test]
fn a_mixed_trigger_requires_safe_replay_of_every_other_lane() {
    for lane in [
        "class = \"write-one-to-clear\"\nwrite-values-only = true\nvalues = [{ name = \"ACK\", value = 1 }]",
        "class = \"hardware-written\"",
        "class = \"unknown\"",
        "class = \"self-clearing\"",
    ] {
        let replacement = format!(
            "name = \"EVENT\"\nbits = \"8\"\n{lane}\n\n[[registers.fields]]\nname = \"PENDING\"\nbits = \"9\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"START\"\nbits = \"16\"\nclass = \"write-trigger\""
        );
        let rust = rendered_with(&[(
            "types/radio.toml",
            "name = \"KEY\"\nbits = \"15:8\"\nclass = \"write-only\"",
            &replacement,
        )]);
        assert!(!rust.contains("pub fn trigger_mixed_start("), "{lane}");
    }
}

#[test]
fn layout_constant_values_are_checked_even_in_runtime_builders() {
    let rendered = rendered_with(&[(
        "layouts/rx-descriptor.toml",
        "bits = \"31\"",
        "bits = \"30:28\"",
    )]);
    let output = run(
        &rendered,
        r#"fn main() {
            let image = layouts::RxDescriptorControl::from_bits(0x8abc_def0)
                .with_owner_constant::<7>();
            assert_eq!(image.owner(), 7);
            assert_eq!(image.bits(), 0xfabc_def0);
        }"#,
    );
    assert!(output.is_empty());
    let errors = refused(
        &rendered,
        r#"fn main() {
            let _ = layouts::RxDescriptorControl::ZERO.with_owner_constant::<8>();
        }"#,
    );
    assert!(errors.contains("E0080"), "{errors}");
    assert!(
        errors.contains("the constant fits the layout field"),
        "{errors}"
    );
}

#[test]
fn hart_geometry_accepts_one_and_three_and_rejects_runtime_constant_misuse() {
    for n in [1, 3] {
        let rust =
            oer_chips_family_facts::generate::hart_geometry(n, 0).expect("synthetic geometry");
        let main = format!(
            r#"fn main() {{
            assert_eq!(HARTS, {n});
            assert_eq!(Hart::ALL.len(), HARTS);
            assert_eq!(Hart::SECONDARIES.len(), HARTS - 1);
            for (raw, hart) in Hart::ALL.into_iter().enumerate() {{
                assert_eq!(Hart::new(raw as u32), Ok(hart));
                assert_eq!(hart.index().get(), raw);
            }}
            assert_eq!(Hart::new({n}), Err(InvalidHart {{ raw: {n} }}));
            assert_eq!(Hart::new(u32::MAX), Err(InvalidHart {{ raw: u32::MAX }}));
        }}"#
        );
        run(&rust, &main);
        let bad = refused(
            &rust,
            &format!("fn main() {{ let _ = Hart::constant::<{n}>(); }}"),
        );
        assert!(bad.contains("E0080"), "{bad}");
        let mismatched = rust.replace(
            &format!("HARTS: usize = {n}"),
            &format!("HARTS: usize = {}", n + 1),
        );
        let bad = refused(&mismatched, "fn main() {}");
        assert!(bad.contains("mismatched types"), "{bad}");
    }
}

#[test]
fn symbolic_hart_fields_preserve_every_other_lane_for_n_one_and_three() {
    for n in [1, 3] {
        let directory = edited(&[
            (
                "chip.toml",
                "chip = \"testchip\"",
                "chip = \"testchip\"\nhart-facts = \"platform.toml\"",
            ),
            (
                "types/counter.toml",
                "name = \"STEP\"",
                "name = \"STEP%s\"\ndim = { count = \"harts\", stride = 4 }",
            ),
        ]);
        std::fs::write(
            directory.path().join("platform.toml"),
            format!(
                "[interrupts]\nboot-hart = 0\nharts = {:?}\n",
                (0..n).collect::<Vec<_>>()
            ),
        )
        .unwrap();
        let (model, policy) = load(directory.path());
        let rust = render(&model, &policy).unwrap();
        run(
            &rust,
            r#"fn main() {
            let mut words = [u32::MAX; 4];
            let mut mode = counter::Mode::host(&mut words).unwrap();
            for hart in Hart::ALL {
                mode.modify_mode(|w| { w.set_step(hart.index(), oer_portable_mmio::Bits::constant::<0>()); });
                let r = mode.read_mode();
                for peer in Hart::ALL {
                    assert_eq!(r.step(peer.index()).get(), if peer.get() <= hart.get() { 0 } else { 15 });
                }
            }
            drop(mode);
            assert_eq!(words[2] >> (4 * HARTS), u32::MAX >> (4 * HARTS), "unassigned bits kept");
        }"#,
        );
    }
}

#[test]
fn instance_families_follow_one_and_three_harts_and_invalid_lists_are_refused() {
    for harts in [vec![0], vec![0, 1, 2], vec![1], vec![0, 0], vec![]] {
        let directory = edited(&[
            (
                "chip.toml",
                "chip = \"testchip\"",
                "chip = \"testchip\"\nhart-facts = \"platform.toml\"",
            ),
            (
                "chip.toml",
                "name = \"COUNTER\"",
                "name = \"COUNTER%s\"\ndim = { count = \"harts\", stride = 0x1000 }",
            ),
        ]);
        std::fs::write(
            directory.path().join("platform.toml"),
            format!("[interrupts]\nboot-hart = 0\nharts = {harts:?}\n"),
        )
        .unwrap();
        let evidence =
            RegisterEvidenceSet::load_all(&[directory.path().join("evidence.toml")]).unwrap();
        let model = ChipModel::load(&directory.path().join("chip.toml"), &evidence);
        if harts == [0] || harts == [0, 1, 2] {
            let model = model.unwrap();
            assert_eq!(
                model
                    .instances()
                    .iter()
                    .filter(|i| i.block_type == "COUNTER")
                    .count(),
                harts.len()
            );
        } else {
            assert!(model.is_err());
        }
    }
}

/// The fixture with `harts` harts and the instances `local` hart-local,
/// loaded and rendered.
fn hart_local(harts: u32, local: &[&str]) -> Result<String, String> {
    hart_local_with(harts, local, &[])
}

/// The fixture in `directory`, loaded and rendered, or the first refusal
/// of the policy's loader or of the renderer.
fn try_render(directory: &Path) -> Result<String, String> {
    let evidence = RegisterEvidenceSet::load_all(&[directory.join("evidence.toml")])
        .map_err(|error| error.to_string())?;
    let model = ChipModel::load(&directory.join("chip.toml"), &evidence)
        .map_err(|error| error.to_string())?;
    let policy = ChipPolicy::load(&directory.join("policy.toml"), &model, &evidence)
        .map_err(|error| error.to_string())?;
    render(&model, &policy).map_err(|error| error.to_string())
}

/// [`hart_local`], with `extra` edits.
fn hart_local_with(
    harts: u32,
    local: &[&str],
    extra: &[(&str, &str, &str)],
) -> Result<String, String> {
    let tags = local
        .iter()
        .map(|instance| {
            (
                format!("name = \"{instance}\"\n"),
                format!("name = \"{instance}\"\nhart-local = {{ sources = [\"SRC_HEADER\"] }}\n"),
            )
        })
        .collect::<Vec<_>>();
    let mut edits = vec![(
        "chip.toml",
        "chip = \"testchip\"",
        "chip = \"testchip\"\nhart-facts = \"platform.toml\"",
    )];
    edits.extend(
        tags.iter()
            .map(|(from, to)| ("chip.toml", from.as_str(), to.as_str())),
    );
    edits.extend_from_slice(extra);
    let directory = edited(&edits);
    std::fs::write(
        directory.path().join("platform.toml"),
        format!(
            "[interrupts]\nboot-hart = 0\nharts = {:?}\n",
            (0..harts).collect::<Vec<_>>()
        ),
    )
    .expect("the platform facts are written");
    try_render(directory.path())
}

#[test]
fn a_hart_local_instance_is_claimed_once_per_hart() {
    for harts in [2, 3] {
        let rust = hart_local(harts, &["RADIO1", "ETM"])
            .unwrap_or_else(|error| panic!("the hart-local fixture renders: {error}"));
        // Standalone parts and the division of a hart-local instance are
        // arrays of HARTS owners; every other part keeps its one owner.
        for member in [
            "pub second: [super::radio::Second; super::HARTS],",
            "pub second_events: [super::radio::SecondEvents; super::HARTS],",
            "pub etm_low: [super::etm::EtmLow; super::HARTS],",
            "pub etm_high: [super::etm::EtmHigh; super::HARTS],",
            "pub task: super::radio::Task,",
            "pub counter: super::counter::Counter,",
        ] {
            assert!(rust.contains(member), "{member}\n{rust}");
        }
        let claims = rust
            .lines()
            .find(|line| line.starts_with("pub type Claims"))
            .expect("the claims are listed");
        for (claim, count) in [
            ("super::radio::SecondClaim", harts),
            ("super::radio::SecondEventsClaim", harts),
            ("super::etm::EtmDivisionClaim", harts),
            ("super::radio::Radio0DivisionClaim", 1),
            ("super::counter::CounterClaim", 1),
        ] {
            assert_eq!(
                claims.matches(&format!("{claim},")).count()
                    + claims.matches(&format!("{claim})")).count(),
                count as usize,
                "{claim} in {claims}"
            );
        }
        // One disjointness check per hart context, each over every block.
        let checks = rust
            .split("const _: () = if !oer_portable_mmio::check::disjoint_blocks(")
            .skip(1)
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), harts as usize, "{rust}");
        for check in &checks {
            for block in [
                "<super::radio::SecondWords as",
                "<super::etm::EtmDivision as",
                "<super::radio::Radio0Division as",
                "<super::counter::CounterWords as",
            ] {
                // Each block once, with its words and its bytes.
                for list in ["WORDS", "BYTES"] {
                    let listed = format!("{block} oer_portable_mmio::Registers>::{list}");
                    assert_eq!(check.matches(&listed).count(), 1, "{listed} in {check}");
                }
            }
        }
        // The program builds, so every per-hart check holds and the
        // partition fills each array with the hart count's owners.
        run(
            &rust,
            &format!(
                r#"fn main() {{
                assert_eq!(HARTS, {harts});
                let _: fn(ownership::Claims) -> ownership::Partitions = ownership::partition;
                let _ = |p: ownership::Partitions| -> ([radio::Second; HARTS], [etm::EtmHigh; HARTS], radio::Task) {{
                    (p.second.second, p.etm.etm_high, p.radio.task)
                }};
            }}"#
            ),
        );
    }
}

#[test]
fn an_instance_without_hart_local_keeps_one_claim_and_one_check() {
    let rust = rendered();
    assert!(!rust.contains("super::HARTS]"), "{rust}");
    assert_eq!(
        rust.matches("oer_portable_mmio::check::disjoint_blocks(")
            .count(),
        1
    );
    // With hart facts but no hart-local instance, the ownership is the same.
    let with_facts = hart_local(2, &[]).expect("the fixture with hart facts renders");
    let ownership = |text: &str| {
        text.split("pub mod ownership {")
            .nth(1)
            .expect("the ownership module")
            .to_owned()
    };
    assert_eq!(ownership(&rust), ownership(&with_facts));
}

#[test]
fn a_shared_part_on_a_hart_local_instance_is_refused() {
    let error = hart_local(2, &["COUNTER"]).expect_err("a shared hart-local part is refused");
    assert!(error.contains("hart-local"), "{error}");
}

#[test]
fn indexed_fields_follow_the_block_prefix_rule() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "name = \"RAD_ENABLE\"\nbits = \"0\"",
        "name = \"RAD_HART%s_ENABLE\"\ndim = { count = 2, stride = 1 }\nbits = \"0\"",
    )]);
    run(
        &rust,
        r#"fn main() {
        let mut memory = [0_u32; 0x40];
        let (mut task, _) = radio::radio0_host(&mut memory).unwrap();
        task.modify_config(|w| { w.set_hart_enable(oer_portable_mmio::Index::constant::<1>(), true); });
        assert!(task.read_config().hart_enable(oer_portable_mmio::Index::constant::<1>()));
        assert!(!task.read_config().hart_enable(oer_portable_mmio::Index::constant::<0>()));
    }"#,
    );
}

fn with_record_word() -> String {
    rendered_with(&[(
        "policy.toml",
        "chip = \"testchip\"\n",
        "chip = \"testchip\"\n\n[[record-words]]\nregister = \"RADIO1.RAD_EVENTS\"\nreason = \"A test record keeps it.\"\nsources = [\"SRC_DRIVER\"]\n",
    )])
}

#[test]
fn a_record_word_is_the_read_value_whole_with_its_unknown_bits() {
    let output = run(
        &with_record_word(),
        r#"
fn main() {
    let mut memory = [0_u32; 0x40];
    memory[0x0c / 4] = 0xfedc_ba98;
    let before = memory;
    {
        let owner = radio::SecondEvents::host(&mut memory).expect("memory holds the block");
        let copy = owner;
        let whole: u32 = owner.read_events().record_word();
        println!("{whole:08x} {:08x}", copy.read_events().record_word());
    }
    assert_eq!(memory, before);
}
"#,
    );
    assert_eq!(output.trim(), "fedcba98 fedcba98");
}

#[test]
fn a_register_not_named_has_no_record_word() {
    let error = refused(
        &with_record_word(),
        r#"
fn main() {
    let mut memory = [0_u32; 0x40];
    let (task, _) = radio::radio0_host(&mut memory).unwrap();
    let _ = task.read_config().record_word();
}
"#,
    );
    assert!(error.contains("no method named `record_word`"), "{error}");
}

#[test]
fn a_record_word_grants_no_status_writer() {
    let error = refused(
        &with_record_word(),
        r#"
fn main() {
    let mut memory = [0_u32; 0x40];
    let mut owner = radio::SecondEvents::host(&mut memory).expect("memory holds the block");
    owner.write_events(|_| {});
}
"#,
    );
    assert!(error.contains("no method named `write_events`"), "{error}");
}

/// Radio 0 hart-local, its two `RAD_HALF%s` bytes held by a shared part
/// with a hart's proof (`Pending`, its stores `pend` and `unpend`), which
/// radio 0's task observes; `more` edits on top.
fn pending_bytes(more: &[(&str, &str, &str)]) -> Result<String, String> {
    let mut edits = vec![
        (
            "types/radio.toml",
            "width = 16\ndim = { count = 2, stride = 2 }",
            "width = 8\ndim = { count = 2, stride = 1 }",
        ),
        (
            "types/radio.toml",
            "name = \"V\"\nbits = \"15:0\"",
            "name = \"V\"\nbits = \"7:0\"",
        ),
        (
            "policy.toml",
            "registers = [\"RAD_HALF%s\", \"RAD_CH0.CNT\"]",
            "registers = [\"RAD_CH0.CNT\"]",
        ),
        (
            "policy.toml",
            "\"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]",
            "\"RAD_CH%s.CONF\", \"RAD_CH1.CNT\"]\nobserves = [\"RAD_HALF%s\"]\ndescription = \"The task reads the pending bytes.\"\nsources = [\"SRC_DRIVER\"]",
        ),
        (
            "policy.toml",
            "[[partitions]]\nname = \"SecondPeripherals\"",
            "[[partitions]]\nname = \"PendingPeripherals\"\nmember = \"pending\"\ndescription = \"Radio 0's pending bytes.\"\n\n\
             [[partitions.parts]]\nname = \"Pending\"\ninstance = \"RADIO0\"\nregisters = [\"RAD_HALF%s\"]\nshared = true\n\
             hart-proof = \"oer_portable_riscv::OnHart\"\ndescription = \"The pending bytes every context stores on its own hart.\"\nsources = [\"SRC_DRIVER\"]\n\n\
             [[partitions]]\nname = \"SecondPeripherals\"",
        ),
        (
            "policy.toml",
            "chip = \"testchip\"\n",
            "chip = \"testchip\"\n\n\
             [[fixed-register-images]]\nname = \"pend\"\nregister = \"RADIO0.RAD_HALF%s\"\nvalue = 0x01\nwhole-word = true\nsources = [\"SRC_DRIVER\"]\n\n\
             [[fixed-register-images]]\nname = \"unpend\"\nregister = \"RADIO0.RAD_HALF%s\"\nvalue = 0x00\nwhole-word = true\nsources = [\"SRC_DRIVER\"]\n",
        ),
    ];
    edits.extend_from_slice(more);
    hart_local_with(2, &["RADIO0"], &edits)
}

#[test]
fn a_shared_part_on_a_hart_local_instance_stores_its_images_with_its_hart_s_proof() {
    let rust =
        pending_bytes(&[]).unwrap_or_else(|error| panic!("the pending fixture renders: {error}"));
    // One type a hart; its stores take that hart's proof, through `&self`.
    for item in [
        "pub struct Pending<const H: u8, P: oer_portable_mmio::Placement",
        "pub fn pend(&self, index: oer_portable_mmio::Index<2>, _hart: &oer_portable_riscv::OnHart<H>)",
        "pub fn unpend(&self, index: oer_portable_mmio::Index<2>, _hart: &oer_portable_riscv::OnHart<H>)",
        "write_byte_shared_indexed::<0x70, 0x1, 2>(index, PEND_IMAGE)",
        "pub pending: (super::radio::Pending<0>, super::radio::Pending<1>,),",
        "type Sharing = oer_portable_mmio::sharing::Shared;",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    // The observer reads the bytes apart from its division, which claims
    // them not; the claims' check lists them once, by their shared block.
    assert!(rust.contains("const OBSERVED_SHARED_BYTES: &'static [usize] = &[0x70, 0x71];"));
    let pending = rust
        .split("pub struct Pending<")
        .nth(1)
        .and_then(|rest| rest.split("pub struct ").next())
        .expect("the pending part");
    assert!(!pending.contains("pub fn read_"), "no read: {pending}");
    assert!(
        !pending.contains("pub fn store_"),
        "no generic store: {pending}"
    );
    run(
        &rust,
        r#"fn main() {
        use oer_portable_mmio::Index;
        use oer_portable_riscv::OnHart;
        let mut memory = [0_u32; 0x40];
        memory[0x70 / 4] = 0xffff_0000;
        {
            let pending = radio::Pending::<1, _>::host(&mut memory).unwrap();
            pending.pend(Index::constant::<1>(), &OnHart::<1>::host());
        }
        // The whole byte, and no other byte of its word.
        assert_eq!(memory[0x70 / 4], 0xffff_0100);
        {
            let (task, _irq) = radio::radio0_host(&mut memory).unwrap();
            assert_eq!(task.read_half(Index::constant::<1>()).v(), 1);
            assert_eq!(task.read_half(Index::constant::<0>()).v(), 0);
        }
        {
            let pending = radio::Pending::<1, _>::host(&mut memory).unwrap();
            pending.unpend(Index::constant::<1>(), &OnHart::<1>::host());
        }
        assert_eq!(memory[0x70 / 4], 0xffff_0000);
    }"#,
    );
    // Another hart's proof, a read, a generic store and the observer's
    // write do not build.
    for (body, error) in [
        (
            "let pending = radio::Pending::<1, _>::host(&mut memory).unwrap();\npending.pend(Index::constant::<1>(), &OnHart::<0>::host());",
            "expected `1`, found `0`",
        ),
        (
            "let pending = radio::Pending::<1, _>::host(&mut memory).unwrap();\nlet _ = pending.read_half(Index::constant::<1>());",
            "no method named `read_half`",
        ),
        (
            "let (mut task, _irq) = radio::radio0_host(&mut memory).unwrap();\ntask.write_half(Index::constant::<1>(), |w| { w.set_v(1); });",
            "no method named `write_half`",
        ),
    ] {
        let refusal = refused(
            &rust,
            &format!(
                "fn main() {{\nuse oer_portable_mmio::Index;\nuse oer_portable_riscv::OnHart;\nlet mut memory = [0_u32; 0x40];\n{body}\n}}"
            ),
        );
        assert!(refusal.contains(error), "{error}: {refusal}");
    }
}

#[test]
fn a_hart_s_proof_belongs_to_a_shared_part_on_a_hart_local_instance() {
    // On an instance every hart reaches as one.
    let error = rejection(&[(
        "policy.toml",
        "registers = [\"COUNT\", \"UPDATE\", \"LATCH\"]\nshared = true",
        "registers = [\"COUNT\", \"UPDATE\", \"LATCH\"]\nshared = true\nhart-proof = \"oer_portable_riscv::OnHart\"",
    )]);
    assert!(error.reason.contains("hart-local"), "{error}");
    // On a part that is not shared.
    let error = pending_bytes(&[(
        "policy.toml",
        "name = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]",
        "name = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]\nhart-proof = \"oer_portable_riscv::OnHart\"\ndescription = \"Mode.\"\nsources = [\"SRC_DRIVER\"]",
    )]);
    let error = error.expect_err("a hart's proof on a part that is not shared is refused");
    assert!(
        error.contains("only a shared part takes a hart's proof"),
        "{error}"
    );
}

#[test]
fn a_shared_part_with_a_hart_s_proof_stores_only_whole_byte_images() {
    // An image that keeps the other bits is no stored image.
    let error = pending_bytes(&[(
        "policy.toml",
        "value = 0x01\nwhole-word = true",
        "value = 0x01",
    )])
    .expect_err("a partial image is refused");
    assert!(error.contains("whole-word"), "{error}");
}

#[test]
fn only_a_shared_part_s_byte_registers_are_observed_apart() {
    // The counter's mode part reading the shared counter's 32-bit word.
    let error = rejection(&[(
        "policy.toml",
        "name = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]",
        "name = \"Mode\"\ninstance = \"COUNTER\"\nregisters = [\"MODE\"]\nobserves = [\"COUNT\"]\ndescription = \"Mode.\"\nsources = [\"SRC_DRIVER\"]",
    )]);
    assert!(
        error
            .reason
            .contains("only a shared part's byte registers are observed apart"),
        "{error}"
    );
}

#[test]
fn a_field_whose_every_value_is_named_reads_as_its_name() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "name = \"RAD_ENABLE\"\nbits = \"0\"\nclass = \"plain\"",
        "name = \"RAD_ENABLE\"\nbits = \"0\"\nclass = \"plain\"\n\
         values = [{ name = \"OFF\", value = 0 }, { name = \"ON\", value = 1 }]",
    )]);
    // Complete: the getter returns the name; no number and no `_variant`.
    assert!(
        rust.contains("pub const fn enable(self) -> Enable {"),
        "{rust}"
    );
    assert!(
        !rust.contains("pub const fn enable(self) -> bool"),
        "{rust}"
    );
    assert!(!rust.contains("enable_variant"), "{rust}");
    // Partial: the number, and the name if it is one.
    assert!(
        rust.contains("pub const fn mode(self) -> oer_portable_mmio::Bits<2> {"),
        "{rust}"
    );
    assert!(
        rust.contains("pub const fn mode_variant(self) -> Option<Mode> {"),
        "{rust}"
    );
    let output = run(
        &rust,
        r#"
fn main() {
    let mut memory = [0_u32; 0x40];
    memory[0] = 1;
    let (task, _irq) = radio::radio0_host(&mut memory).expect("memory holds the block");
    println!("{:?}", task.read_config().enable());
}
"#,
    );
    assert_eq!(output.trim(), "On");
}

/// The chip's definitions: a dictionary `GATE` and a flag set `STATE_MAP`
/// of three flags in four bits, bit 3 named by none.
const DEFINITIONS: &str = "schema = 1\n\n[[dictionaries]]\nname = \"GATE\"\n\
    description = \"A gate that is closed or open.\"\nwidth = 1\n\
    values = [{ name = \"CLOSED\", value = 0, sources = [\"SRC_HEADER\"] }, \
    { name = \"OPEN\", value = 1, sources = [\"SRC_HEADER\"] }]\n\
    sources = [\"SRC_HEADER\"]\n\n[[flags]]\nname = \"STATE_MAP\"\nflag = \"STATE\"\n\
    description = \"The states a clock stays on in.\"\nwidth = 4\n\
    bits = [{ name = \"SLEEP\", bit = 0, sources = [\"SRC_HEADER\"] }, \
    { name = \"MODEM\", bit = 1, sources = [\"SRC_HEADER\"] }, \
    { name = \"ACTIVE\", bit = 2, sources = [\"SRC_HEADER\"] }]\n\
    sources = [\"SRC_HEADER\"]\n";

/// The fixture with the chip's definitions, `RAD_CONFIG.STATES` (15:12)
/// holding the flag set and `RAD_ENABLE` naming the chip's `GATE`.
fn with_definitions() -> String {
    rendered_with(&[
        ("dictionaries.toml", "", DEFINITIONS),
        (
            "chip.toml",
            "schema = 4\n",
            "schema = 4\ndefinitions = \"dictionaries.toml\"\n",
        ),
        (
            "types/radio.toml",
            "name = \"RAD_ENABLE\"\nbits = \"0\"\nclass = \"plain\"",
            "name = \"RAD_ENABLE\"\nbits = \"0\"\nclass = \"plain\"\nvalues = { dictionary = \"GATE\" }\n\n\
             [[registers.fields]]\nname = \"STATES\"\nbits = \"15:12\"\nclass = \"plain\"\nflags = \"STATE_MAP\"",
        ),
    ])
}

#[test]
fn a_flag_set_read_is_exactly_its_flags_and_carries_to_another_word() {
    let rust = with_definitions();
    let output = run(
        &rust,
        r#"
use dictionaries::{State, StateMap};
fn main() {
    let mut memory = [0_u32; 128];
    // CONFIG: STATES (15:12) holds SLEEP and ACTIVE; the fields beside it,
    // below (RAD_ENABLE, LEVEL) and above (QUIET at 17), are set too.
    memory[0] = 0x0002_5031;
    // Radio 1's CONFIG: every field set but STATES.
    let mut second = [0x0002_0031_u32; 128];
    {
        let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
        let read = task.read_config().states();
        println!("{}", read == StateMap::EMPTY.with(State::Sleep).with(State::Active));
        let mut radio1 = radio::Second::host(&mut second).expect("host block");
        radio1.modify_config(|w| {
            w.set_states(read);
        });
    }
    println!("{:x}", second[0]);
}
"#,
    );
    // Radio 1's STATES takes SLEEP and ACTIVE; its other fields stay, but
    // the zero-only QUIET (17), which a modify writes zero.
    assert_eq!(output.trim(), "true\n5031");
}

#[test]
fn the_chips_definitions_render_once_in_the_root() {
    let rust = with_definitions();
    assert_eq!(rust.matches("pub enum Gate {").count(), 1, "{rust}");
    assert_eq!(
        rust.matches("pub struct StateMap(u32);").count(),
        1,
        "{rust}"
    );
    assert_eq!(rust.matches("pub enum State {").count(), 1, "{rust}");
    assert!(
        rust.contains("pub const fn enable(self) -> super::super::dictionaries::Gate {"),
        "{rust}"
    );
    assert!(
        rust.contains("pub const fn states(self) -> super::super::dictionaries::StateMap {"),
        "{rust}"
    );
    // A set has no reader of one value: its flags are asked one by one.
    assert!(!rust.contains("states_variant"), "{rust}");
}

#[test]
fn a_flag_set_adds_its_flags_and_keeps_the_bits_no_flag_names() {
    let rust = with_definitions();
    let output = run(
        &rust,
        r#"
use dictionaries::{State, StateMap};
fn main() {
    // Bits 13 (MODEM) and 15 (named by no flag) are set.
    let mut memory = [0x0000_a000_u32; 128];
    {
        let (mut task, _) = radio::radio0_host(&mut memory).expect("host block");
        let read = task.read_config().states();
        println!("{} {} {}", read.contains(State::Sleep), read.contains(State::Modem), read.contains(State::Active));
        task.modify_config(|w| {
            w.set_states(w.states().with(State::Active));
        });
        let read = task.read_config().states();
        println!("{} {}", read.contains(State::Active), read == StateMap::EMPTY);
        task.modify_config(|w| {
            w.set_states(StateMap::EMPTY.with(State::Sleep).union(StateMap::EMPTY));
        });
    }
    println!("{:x}", memory[0] & 0xf000);
}
"#,
    );
    // ACTIVE joins the read set and bit 15 stays as read; the set written
    // whole holds only what it names.
    assert_eq!(output.trim(), "false true false\ntrue false\n1000");
}

/// A two-byte element and an array of four of it.
const PAIR: &str = r#"schema = 1

[layout]
name = "PAIR"
description = "Two bytes."
size = 2
align = 1

[layout.review]
sources = ["SRC_HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[layout.bytes]]
name = "LOW"
offset = 0
size = 1
access = "owned"

[[layout.bytes.fields]]
name = "LOW_VALUE"
bits = "7:0"
writer = "cpu"
description = "The low byte."

[[layout.bytes]]
name = "HIGH"
offset = 1
size = 1
access = "owned"

[[layout.bytes.fields]]
name = "HIGH_VALUE"
bits = "7:0"
writer = "cpu"
description = "The high byte."
"#;
const PAIRS: &str = r#"schema = 1

[layout]
name = "PAIRS"
description = "Four pairs."
size = 8
align = 1

[layout.review]
sources = ["SRC_HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[layout.array]
element = "PAIR"
count = 4
"#;

#[test]
fn a_byte_array_s_entry_is_its_index_s_element_and_none_past_the_end() {
    let rust = rendered_with(&[
        ("layouts/pair.toml", "", PAIR),
        ("layouts/pairs.toml", "", PAIRS),
        (
            "chip.toml",
            "layouts = [\"layouts/rx-descriptor.toml\"]",
            "layouts = [\"layouts/rx-descriptor.toml\", \"layouts/pair.toml\", \"layouts/pairs.toml\"]",
        ),
    ]);
    let output = run(
        &rust,
        r#"
use layouts::{Pairs, PairsMut, PairLow};
fn main() {
    let mut bytes = [0x10_u8, 0x11, 0x20, 0x21, 0x30, 0x31, 0x40, 0x41];
    let pairs = Pairs::new(&bytes);
    for index in [0, 1, 3] {
        let pair = pairs.entry(index).expect("an element");
        println!("{:x} {:x}", pair.low().low_value(), pair.high().high_value());
    }
    println!("{}", pairs.entry(4).is_none());
    {
        let mut pairs = PairsMut::new(&mut bytes);
        for index in [0, 2, 3] {
            pairs
                .entry(index)
                .expect("an element")
                .set_low(PairLow::from_bits(0xa0 + index as u8));
        }
        println!("{}", pairs.entry(4).is_none());
    }
    println!("{:x?}", bytes);
}
"#,
    );
    assert_eq!(
        output.trim(),
        "10 11\n20 21\n40 41\ntrue\ntrue\n[a0, 11, 20, 21, a2, 31, a3, 41]"
    );
}

#[test]
fn fields_of_one_dictionary_share_its_enum() {
    let rust = rendered_with(&[
        (
            "types/radio.toml",
            "schema = 3\n",
            "schema = 3\n\n[[dictionaries]]\nname = \"LANE\"\n\
             description = \"A lane that is shut or open.\"\nwidth = 1\n\
             values = [{ name = \"SHUT\", value = 0, sources = [\"SRC_HEADER\"] }, \
             { name = \"OPEN\", value = 1, sources = [\"SRC_HEADER\"] }]\n\
             sources = [\"SRC_HEADER\"]\n",
        ),
        (
            "types/radio.toml",
            "name = \"RX\"\nbits = \"0\"\nclass = \"plain\"",
            "name = \"RX\"\nbits = \"0\"\nclass = \"plain\"\nvalues = { dictionary = \"LANE\" }",
        ),
        (
            "types/radio.toml",
            "name = \"TX\"\nbits = \"1\"\nclass = \"plain\"",
            "name = \"TX\"\nbits = \"1\"\nclass = \"plain\"\nvalues = { dictionary = \"LANE\" }",
        ),
    ]);
    assert_eq!(rust.matches("pub mod dictionaries {").count(), 1, "{rust}");
    assert_eq!(rust.matches("pub enum Lane {").count(), 1, "{rust}");
    // The fields have no enum of their own: both read and take the dictionary's.
    assert!(!rust.contains("pub enum Rx {"), "{rust}");
    assert!(!rust.contains("pub enum Tx {"), "{rust}");
    assert!(
        rust.contains("pub const fn rx(self) -> super::dictionaries::Lane {"),
        "{rust}"
    );
    assert!(
        rust.contains("pub const fn tx(self) -> super::dictionaries::Lane {"),
        "{rust}"
    );
    let output = run(
        &rust,
        r#"
fn main() {
    let rx: fn(radio::enable::R) -> radio::dictionaries::Lane = radio::enable::R::rx;
    let tx: fn(radio::enable::R) -> radio::dictionaries::Lane = radio::enable::R::tx;
    let _ = (rx, tx);
    let lanes = [
        radio::dictionaries::Lane::from_field(0b10),
        radio::dictionaries::Lane::from_field(0b10 >> 1),
    ];
    println!("{lanes:?}");
}
"#,
    );
    assert_eq!(output.trim(), "[Shut, Open]");
}

#[test]
fn a_named_value_of_a_field_open_for_writing_converts_into_its_setters_type() {
    let rust = rendered_with(&[
        (
            "types/radio.toml",
            "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"",
            "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"\n\
             values = [{ name = \"FIRST\", value = 1 }, { name = \"LAST\", value = 0xff }]",
        ),
        (
            "types/radio.toml",
            "write-range = { min = 1, max = 5 }",
            "write-range = { min = 1, max = 5 }\n\
             values = [{ name = \"LOW\", value = 1 }, { name = \"HIGH\", value = 5 }]",
        ),
    ]);
    // A native setter: the name becomes the field's number, untruncated.
    assert!(rust.contains("impl From<Argument> for u8 {"), "{rust}");
    // A bounded setter whose range holds every name: the range type.
    assert!(rust.contains("impl From<Level> for LevelRange {"), "{rust}");
    // A field written only by name takes the enum itself: no conversion.
    assert!(!rust.contains("impl From<Mode> for"), "{rust}");
    let output = run(
        &rust,
        r#"
fn main() {
    let argument: u8 = radio::command::Argument::Last.into();
    let level: radio::config::LevelRange = radio::config::Level::High.into();
    println!("{argument} {}", level.get());
}
"#,
    );
    assert_eq!(output.trim(), "255 5");
}

#[test]
fn a_named_value_outside_the_write_range_has_no_conversion() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "write-range = { min = 1, max = 5 }",
        "write-range = { min = 1, max = 5 }\n\
         values = [{ name = \"OFF\", value = 0 }, { name = \"HIGH\", value = 5 }]",
    )]);
    assert!(rust.contains("pub enum Level {"), "{rust}");
    assert!(!rust.contains("impl From<Level> for"), "{rust}");
}

#[test]
fn an_eight_or_sixteen_bit_field_is_masked_to_its_width_before_its_cast() {
    let rust = rendered_with(&[(
        "types/counter.toml",
        "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"",
        "name = \"STEP\"\nbits = \"23:16\"\nclass = \"plain\"\n\n\
         [[registers.fields]]\nname = \"SPAN\"\nbits = \"15:0\"\nclass = \"plain\"",
    )]);
    // The cast keeps the field's bits only: the mask proves its range, to
    // the cast lints as to a reader, and no unmasked cast is left.
    assert!(rust.contains("((self.0 >> 16) & 0xff) as u8"), "{rust}");
    assert!(rust.contains("(self.0 & 0xffff) as u16"), "{rust}");
    assert!(!rust.contains("(self.0 >> 16) as u8"), "{rust}");
    assert!(!rust.contains("self.0 as u16"), "{rust}");
}

#[test]
fn an_open_dictionary_converts_into_its_fields_native_setter_type() {
    let rust = rendered_with(&[
        (
            "types/radio.toml",
            "schema = 3\n",
            "schema = 3\n\n[[dictionaries]]\nname = \"RATE\"\n\
             description = \"A rate byte with two named values.\"\nwidth = 8\n\
             values = [{ name = \"ORDINARY\", value = 0x80, sources = [\"SRC_HEADER\"] }, \
             { name = \"EXTENDED\", value = 0xa0, sources = [\"SRC_HEADER\"] }]\n\
             sources = [\"SRC_HEADER\"]\n",
        ),
        (
            "types/radio.toml",
            "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"",
            "name = \"ARGUMENT\"\nbits = \"15:8\"\nclass = \"write-only\"\nvalues = { dictionary = \"RATE\" }",
        ),
    ]);
    assert!(rust.contains("impl From<Rate> for u8 {"), "{rust}");
    let output = run(
        &rust,
        r#"
fn main() {
    let rate: u8 = radio::dictionaries::Rate::Extended.into();
    println!("{rate}");
}
"#,
    );
    assert_eq!(output.trim(), "160");
}

#[test]
fn a_complete_dictionary_has_no_conversion_into_a_number() {
    let rust = rendered_with(&[(
        "types/radio.toml",
        "schema = 3\n",
        "schema = 3\n\n[[dictionaries]]\nname = \"LANE\"\n\
         description = \"A lane that is shut or open.\"\nwidth = 1\n\
         values = [{ name = \"SHUT\", value = 0, sources = [\"SRC_HEADER\"] }, \
         { name = \"OPEN\", value = 1, sources = [\"SRC_HEADER\"] }]\n\
         sources = [\"SRC_HEADER\"]\n",
    )]);
    assert!(rust.contains("pub enum Lane {"), "{rust}");
    assert!(!rust.contains("impl From<Lane> for"), "{rust}");
}

/// The model's load error of the fixture with `edits`.
fn model_rejection(edits: &[(&str, &str, &str)]) -> String {
    let directory = edited(edits);
    let evidence = RegisterEvidenceSet::load_all(&[directory.path().join("evidence.toml")])
        .expect("the fixture evidence loads");
    match ChipModel::load(&directory.path().join("chip.toml"), &evidence) {
        Ok(_) => panic!("the edited fixture loads"),
        Err(error) => error.to_string(),
    }
}

const LATCH_VALUE: &str = "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"plain\"";
const COUNT_COUNT: &str = "name = \"COUNT\"\nbits = \"31:0\"\nclass = \"read-only\"";

#[test]
fn a_carried_layout_gets_a_typed_setter_and_getter_beside_the_field_s_own() {
    let rendered = rendered_with(&[
        (
            "types/counter.toml",
            LATCH_VALUE,
            "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"plain\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\"]",
        ),
        (
            "types/counter.toml",
            COUNT_COUNT,
            "name = \"COUNT\"\nbits = \"31:0\"\nclass = \"read-only\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\"]",
        ),
    ]);
    assert!(rendered.contains(
        "pub fn set_value_control(&mut self, value: super::super::layouts::RxDescriptorControl) -> &mut Self"
    ));
    assert!(rendered.contains(
        "pub const fn value_control(self) -> super::super::layouts::RxDescriptorControl"
    ));
    // A read-only field reads as the layout and has no setter.
    assert!(rendered.contains(
        "pub const fn count_control(self) -> super::super::layouts::RxDescriptorControl"
    ));
    assert!(!rendered.contains("pub fn set_count_control("));
    // The field's own accessors stay.
    assert!(rendered.contains("pub fn set_value(&mut self, value: u32) -> &mut Self"));
    // Layout ranges start from their generated zero.
    assert!(rendered.contains("pub const ZERO: Self = Self(0);"));
}

#[test]
fn a_carried_layout_wider_than_its_field_or_unknown_is_refused() {
    let wide = model_rejection(&[(
        "types/radio.toml",
        "name = \"LOW\"\nbits = \"23:0\"\nclass = \"plain\"",
        "name = \"LOW\"\nbits = \"23:0\"\nclass = \"plain\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\"]",
    )]);
    assert!(
        wide.contains("reaches bit 31, past the field's 24 bits"),
        "{wide}"
    );
    let unknown = model_rejection(&[(
        "types/counter.toml",
        LATCH_VALUE,
        "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"plain\"\ncarries = [\"RX_DESCRIPTOR.STATUS\"]",
    )]);
    assert!(unknown.contains("names no layout range"), "{unknown}");
    let twice = model_rejection(&[(
        "types/counter.toml",
        LATCH_VALUE,
        "name = \"VALUE\"\nbits = \"31:0\"\nclass = \"plain\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\", \"RX_DESCRIPTOR.CONTROL\"]",
    )]);
    assert!(twice.contains("carried twice"), "{twice}");
    let trigger = model_rejection(&[(
        "types/counter.toml",
        "name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-trigger\"",
        "name = \"UPDATE\"\nbits = \"0\"\nclass = \"write-trigger\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\"]",
    )]);
    assert!(
        trigger.contains("only a plain, write-only or read-only field"),
        "{trigger}"
    );
}

#[test]
fn a_carried_word_is_cut_to_its_field_and_read_back_to_its_range_s_bits() {
    let rendered = rendered_with(&[
        (
            "layouts/rx-descriptor.toml",
            "name = \"OWNER\"\nbits = \"31\"",
            "name = \"OWNER\"\nbits = \"3\"",
        ),
        (
            "types/counter.toml",
            "name = \"STEP\"\nbits = \"3:0\"\nclass = \"plain\"",
            "name = \"STEP\"\nbits = \"7:4\"\nclass = \"plain\"\ncarries = [\"RX_DESCRIPTOR.CONTROL\"]",
        ),
    ]);
    // The setter writes the word's four low bits alone into bits 7:4.
    assert!(
        rendered
            .contains("self.0 = (self.0 & 0xffffff0f) | ((u32::from(value.bits()) & 0xf) << 4);"),
        "{rendered}"
    );
    // The getter hands the range only the bit its field reaches.
    assert!(rendered.contains(
        "super::super::layouts::RxDescriptorControl::from_bits(((self.0 >> 4) & 0x8) as _)"
    ));
}
