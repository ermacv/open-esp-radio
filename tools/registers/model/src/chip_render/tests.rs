//! The thin PAC renderer on a synthetic chip (`fixture/`): every field
//! class, register construct and ownership flag. Rendered code is compiled
//! against the workspace's `oer-mmio` and run on host memory; compile-fail
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
             [dependencies]\noer-mmio = {{ path = {:?} }}\n\
             bytemuck = {{ version = \"1.25.2\", default-features = false, features = [\"derive\", \"min_const_generics\"] }}\n\
             [workspace]\n",
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
        "pub fn initialize_vendor_words_word_0060_opaque(&mut self)",
        "pub fn mask_and_clear_events(&mut self)",
        "pub fn sample_radio_events(&self)",
        "pub fn acknowledge_radio_events(",
        "pub fn publish_low(&mut self, low: oer_mmio::Bits<24>)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    // A plain fixed image, a plain sequence step and a zero write are HAL
    // code over accessors.
    assert!(!rust.contains("set_slow_mode"), "{rust}");
    assert!(!rust.contains("initialize_vendor_words_rad_slot"), "{rust}");
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
        "fn main() { println!(\"{:?}\", <radio::Irq as oer_mmio::InterruptSources>::SOURCES); }",
    );
    assert_eq!(output.trim(), "[0, 3]");
    let errors = refused(
        &rendered,
        "fn main() { let _ = <radio::Irq as oer_mmio::InterruptSource>::SOURCE; }",
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
    // An address field takes its typed address, never an integer.
    (
        "fn pointer(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_desc(|w| { w.set_ptr(0x4080_0000); }); }",
        "expected `DmaRef<RxDescriptor>`, found integer",
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
    (
        "policy.toml",
        "[[zero-register-writes]]\nname = \"clear_command\"\nregister = \"RADIO0.RAD_COMMAND\"\nsources = [\"SRC_DRIVER\"]\n",
        "",
    ),
];

/// The shapes' accessors on host memory.
const SHAPES_BEHAVIOUR: &str = r#"
fn hex(word: u32) -> String {
    format!("{word:08x}")
}

/// A typed address reaches the pointer's setter; never called, as only
/// the hardware's placement makes one.
fn publish(task: &mut radio::Task<oer_mmio::Host<'_>>, next: oer_mmio::DmaRef<layouts::RxDescriptor>) {
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
        println!("fifo {:x} {:x}", task.pop_ch_cnt(oer_mmio::Index::constant::<0>()), task.pop_ch_cnt(oer_mmio::Index::constant::<1>()));
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
        shared.trigger_update_update(oer_mmio::Bits::new(9).expect("4 bits"));
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
        "fn port(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.write_desc(0xdead_beef); }",
        // ARGUMENT is never written zero: the write takes it.
        "fn omitted(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.write_command(|w: &mut radio::command::W| { w.set_opcode(radio::command::Opcode::Run); }); }",
        // The high field takes no integer.
        "fn high(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_high(|w| { w.set_address_high(0x1234); }); }",
        // The doorbell is rung only by its trigger.
        "fn ring(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.modify_rxctl(|w| { w.set_append(true); }); }",
        // The step's write range wins over its complete enumeration.
        "fn step(mode: &mut counter::Mode<oer_mmio::Host<'_>>) { mode.modify_mode(|w| { w.set_step(counter::mode::Step::Off); }); }",
        // A zero-only trigger has no trigger.
        "fn quiet(task: &mut radio::Task<oer_mmio::Host<'_>>) { task.trigger_config_start(); }",
        "fn main() {}",
    ]
    .join("\n");
    let edits = SHAPES
        .iter()
        .copied()
        .chain([
            (
                "policy.toml",
                "[[pointer-fields]]\nname = \"descriptor\"",
                "[[full-register-writes]]\nname = \"descriptor\"\npointer = true",
            ),
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
            "[[pointer-fields]]\nname = \"descriptor\"\nfield = \"RADIO0.RAD_DESC.PTR\"",
            "[[field-or-modifies]]\nname = \"or_descriptor\"\nfields = [{ field = \"RADIO0.RAD_DESC.PTR\", source-bit-offset = 0 }]",
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
fn or(task: &mut radio::Task<oer_mmio::Host<'_>>, next: oer_mmio::DmaRef<layouts::RxDescriptor>) {
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
            t.go().level(oer_mmio::Bits::new(5).expect("3 bits"));
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
        "pub fn or_descriptor(&mut self, value: oer_mmio::DmaRef<super::layouts::RxDescriptor>)",
        "self.0.write::<0x5c>(self.0.read::<0x5c>() | value.bits());",
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
        radio::Second::<oer_mmio::Host<'_>>::SECOND_IMAGE_OFFSETS,
        radio::SecondEvents::<oer_mmio::Host<'_>>::SECOND_IMAGE_OFFSETS,
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
