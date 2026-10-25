//! The thin PAC renderer on a synthetic chip (`fixture/`): every field
//! class, register construct and ownership flag. Rendered code is compiled
//! against the workspace's `oer-mmio` and run on host memory; compile-fail
//! programs show what an owner cannot do.

use std::path::{Path, PathBuf};

use super::{names, render, render_parts};
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

/// The fixture's blocks and owners, without the `layouts` module, which
/// needs the PAC's `bytemuck`.
fn rendered() -> String {
    let (model, policy) = load(&fixture());
    render_parts(&model, &policy)
        .unwrap_or_else(|error| panic!("the fixture renders: {error}"))
        .0
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
    directory
}

/// Builds `rendered` with the workspace's `oer-mmio` and `main` into a
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
    std::fs::write(
        directory.join("Cargo.toml"),
        format!(
            "[package]\nname = \"rendered\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\noer-mmio = {{ path = {:?} }}\n[workspace]\n",
            mmio.display().to_string()
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
        "pub fn mask_and_clear_events(&mut self)",
        "pub fn sample_radio_events(&self)",
        "pub fn acknowledge_radio_events(",
        "pub fn publish_low(&mut self, low: oer_mmio::Bits<24>)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    // A plain fixed image and a zero write are HAL code over accessors.
    assert!(!rust.contains("set_slow_mode"), "{rust}");
    assert!(!rust.contains("clear_command"), "{rust}");
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
        "pub fn read_slot(&self, index: oer_mmio::Index<4>) -> slot::R",
        "pub fn read_ch_conf(&self, index: oer_mmio::Index<2>) -> ch_conf::R",
        "pub fn read_ch1_cnt(&self) -> ch_cnt::R",
        "pub fn modify_word_0060_opaque(",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
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
        rust.contains("pub fn read_slot_n(&self, index: oer_mmio::Index<4>) -> slot_n::R"),
        "{rust}"
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
        task.modify_slot(oer_mmio::Index::constant::<2>(), |w| {
            w.set_value(0x1234);
        });
        task.modify_ch_conf(oer_mmio::Index::new(1).expect("index 1"), |w| {
            w.set_en(true);
        });
        println!("ch1 cnt {:x}", task.read_ch1_cnt().n());
        task.store_vendor_word();
        task.publish_low(oer_mmio::Bits::new(0x00ab_cdef).expect("24 bits"));
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
    println!("source {}", <radio::Irq as oer_mmio::InterruptSource>::SOURCE);

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

fn memory_word(_low: &etm::EtmLow<oer_mmio::Host<'_>>) -> u32 {
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
        "fn observed(irq: &mut radio::Irq<oer_mmio::Host<'_>>) { irq.modify_config(|_| {}); }",
        "no method named `modify_config`",
    ),
    // The clear word of a snapshot is written only by its acknowledgement.
    (
        "fn clear(irq: &mut radio::Irq<oer_mmio::Host<'_>>) { irq.trigger_event_clr_rx(); }",
        "no method named `trigger_event_clr_rx`",
    ),
    // A write-only word has no read.
    (
        "fn command(task: &radio::Task<oer_mmio::Host<'_>>) { task.read_command(); }",
        "no method named `read_command`",
    ),
    // A readable word with a write-only lane has no generic writer.
    (
        "fn mixed(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_mixed(|_| {}); }",
        "no method named `modify_mixed`",
    ),
    // A word the hardware also writes is only read.
    (
        "fn hardware(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_hw(|_| {}); }",
        "no method named `modify_hw`",
    ),
    // An unknown field has no accessor.
    (
        "fn vague(task: &radio::Task<oer_mmio::Host<'_>>) { task.read_vague(); }",
        "no method named `read_vague`",
    ),
    // A word a read changes has none either.
    (
        "fn state(task: &radio::Task<oer_mmio::Host<'_>>) { task.read_state(); }",
        "no method named `read_state`",
    ),
    // An address field has no integer setter.
    (
        "fn pointer(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_desc(|_| {}); }",
        "no method named `modify_desc`",
    ),
    // A field written only with its values takes no integer.
    (
        "fn values(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_config(|w| { w.set_mode(1); }); }",
        "expected `Mode`, found integer",
    ),
    // A bounded field takes its bounded type.
    (
        "fn bounded(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_config(|w| { w.set_level(3); }); }",
        "expected `LevelRange`, found integer",
    ),
    // An unclaimed word has no owner.
    (
        "fn unclaimed(task: &radio::Task<oer_mmio::Host<'_>>) { task.read_ch0_cnt(); }",
        "no method named `read_ch0_cnt`",
    ),
    // A part strobes only its own trigger fields.
    (
        "fn foreign(low: &mut etm::EtmLow<oer_mmio::Host<'_>>) { low.trigger_channel_enable_set_ch2(); }",
        "no method named `trigger_channel_enable_set_ch2`",
    ),
    // ... and holds only its own elements.
    (
        "fn element(high: &mut etm::EtmHigh<oer_mmio::Host<'_>>) { high.modify_ch0_event(|_| {}); }",
        "no method named `modify_ch0_event`",
    ),
    // A shared word is stored whole, never read and written back.
    (
        "fn shared(counter: &mut counter::Counter<oer_mmio::Host<'_>>) { counter.modify_latch(|_| {}); }",
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
