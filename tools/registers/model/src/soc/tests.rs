use std::collections::{BTreeMap, BTreeSet};

use super::{RegisterApi, SocInputs, SocRender, StrictnessOverlay, render_soc};
use crate::HeaderTree;

const PIN: &str = "schema = 1\nesp-idf = \"4d59230ddff16327812782151ef0afef202dc6d7\"\n\
                   esp-hal = \"1f646c5af4f4ca2774f493783581237fea2379dc\"\n";
const REG: &str = "components/soc/test/register/soc/timer_reg.h";
const LAYOUT: &str = "components/soc/test/register/soc/timer_struct.h";
const INTERRUPTS: &str = "components/soc/test/include/soc/interrupts.h";

fn register(name: &str, offset: u32, access: &str, fields: &str) -> String {
    format!(
        "<register><name>{name}</name><addressOffset>0x{offset:x}</addressOffset>\
         <size>32</size><access>{access}</access><fields>{fields}</fields></register>"
    )
}

fn field(name: &str, offset: u32, width: u32, access: &str) -> String {
    format!(
        "<field><name>{name}</name><bitOffset>{offset}</bitOffset><bitWidth>{width}</bitWidth>\
         <access>{access}</access></field>"
    )
}

/// A vendor SVD of `TIMER` (with `registers` and one interrupt) and
/// `TIMER1`, derived from it.
fn vendor(registers: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <device schemaVersion=\"1.3\"><name>VENDOR</name><addressUnitBits>8</addressUnitBits>\
         <width>32</width><size>32</size><access>read-write</access><peripherals>\
         <peripheral><name>TIMER</name><description>Timer [0]\n  - one</description>\
         <baseAddress>0x60000000</baseAddress>\
         <interrupt><name>TIMER0</name><value>9</value></interrupt>\
         <registers>{registers}</registers></peripheral>\
         <peripheral derivedFrom=\"TIMER\"><name>TIMER1</name><baseAddress>0x60001000</baseAddress>\
         <interrupt><name>TIMER1</name><value>10</value></interrupt></peripheral>\
         </peripherals></device>"
    )
}

/// A curated SVD that describes the word at 0x60000020.
fn curated() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <device schemaVersion=\"1.3\"><name>CURATED</name><addressUnitBits>8</addressUnitBits>\
         <width>32</width><size>32</size><access>read-write</access><peripherals>\
         <peripheral><name>TIMER_RADIO</name><baseAddress>0x60000020</baseAddress>\
         <registers>{}</registers></peripheral></peripherals></device>",
        register(
            "WORD",
            0,
            "read-write",
            &field("VALUE", 0, 32, "read-write")
        )
    )
}

/// One field line of a pinned `*_reg.h`: name, access class, low bit,
/// width, description.
type HeaderField<'a> = (&'a str, &'a str, u32, u32, &'a str);

/// A `*_reg.h` in ESP-IDF's layout: each register's definition line and
/// its fields' comment lines, all named after the `TIMER_` prefix.
fn reg_h(registers: &[(&str, u32, &[HeaderField<'_>])]) -> String {
    let mut text = String::from("#pragma once\n\n");
    for (name, offset, fields) in registers {
        text.push_str(&format!(
            "/** TIMER_{name}_REG register\n *  The {name} register.\n */\n\
             #define TIMER_{name}_REG (DR_REG_TIMER_BASE + 0x{offset:x})\n"
        ));
        for (field, access, low, width, description) in *fields {
            let high = low + width - 1;
            text.push_str(&format!(
                "/** TIMER_{field} : {access}; bitpos: [{high}:{low}]; default: 0;\n\
                 *  {description}\n */\n#define TIMER_{field}_S  {low}\n"
            ));
        }
        text.push('\n');
    }
    text
}

/// The 1-based line of `text` holding `needle`.
fn line(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|index| index + 1)
        .unwrap_or_else(|| panic!("{needle} is in the header"))
}

/// Pinned files by path.
struct Tree(BTreeMap<String, String>);

impl HeaderTree for Tree {
    fn read(&self, path: &str) -> crate::Result<String> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| crate::Error::message(format!("{path} is not pinned")))
    }
    fn list(&self, _directory: &str) -> crate::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

/// The pinned tree: `reg` as the timer header, a device layout reserving
/// the words at 0x40..0x48, and an interrupts header with source 84 and
/// the source count.
fn tree(reg: &str) -> Tree {
    Tree(BTreeMap::from([
        (REG.to_owned(), reg.to_owned()),
        (
            LAYOUT.to_owned(),
            "typedef struct timer_dev_t {\n    volatile timer_config_reg_t config;\n    \
             uint32_t reserved_040[2];\n} timer_dev_t;\n"
                .to_owned(),
        ),
        (
            INTERRUPTS.to_owned(),
            "typedef enum {\n    ETS_CPU_INTR_FROM_CPU_0_SOURCE = 84,\n    \
             ETS_MAX_INTR_SOURCE,\n} periph_interrupt_t;\n"
                .to_owned(),
        ),
    ]))
}

/// Both timer blocks mapped to the pinned timer header.
fn mapping() -> String {
    ["TIMER", "TIMER1"]
        .iter()
        .map(|peripheral| {
            format!(
                "[[headers]]\nperipheral = \"{peripheral}\"\nregisters = \"{REG}\"\n\
                 layout = \"{LAYOUT}\"\n"
            )
        })
        .collect()
}

fn soc(registers: &str, reg: &str, overlay: &str) -> crate::Result<SocRender> {
    let overlay = StrictnessOverlay::parse(&format!("{PIN}{}{overlay}", mapping()))?;
    render_soc(&SocInputs {
        vendor_svd: &vendor(registers),
        vendor_path: "chips/test/registers/upstream/vendor.svd",
        overlay: &overlay,
        curated_svd: &curated(),
        curated_typed: &BTreeSet::new(),
        headers: &tree(reg),
    })
}

fn api(render: &SocRender, register: &str) -> RegisterApi {
    api_of(render, "TIMER", register)
}

fn api_of(render: &SocRender, peripheral: &str, register: &str) -> RegisterApi {
    render
        .verdicts
        .iter()
        .find(|verdict| verdict.peripheral == peripheral && verdict.register == register)
        .unwrap_or_else(|| panic!("{peripheral}.{register} has a verdict"))
        .api
}

fn reasons(render: &SocRender, register: &str) -> String {
    render
        .verdicts
        .iter()
        .find(|verdict| verdict.peripheral == "TIMER" && verdict.register == register)
        .map(|verdict| verdict.reasons.join("; "))
        .unwrap_or_default()
}

/// A register whose every bit is a declared field.
fn config() -> String {
    register(
        "CONFIG",
        0,
        "read-write",
        &(field("ENABLE", 0, 1, "read-write") + &field("DIVIDER", 1, 31, "read-write")),
    )
}

fn config_h() -> String {
    reg_h(&[(
        "CONFIG",
        0,
        &[
            ("ENABLE", "R/W", 0, 1, "Enables the timer."),
            ("DIVIDER", "R/W", 1, 31, "The clock divider."),
        ],
    )])
}

#[test]
fn a_register_whose_header_fields_are_all_read_write_keeps_its_field_api() {
    let render = soc(&config(), &config_h(), "").expect("renders");
    assert_eq!(api(&render, "CONFIG"), RegisterApi::Full);
    assert!(
        render.rust.contains("pub fn modify_config("),
        "{}",
        render.rust
    );
    assert!(render.rust.contains("pub fn read_config("));
    assert!(!render.rust.contains("unsafe"));
}

#[test]
fn a_peripheral_without_a_pinned_header_is_only_read() {
    let overlay = StrictnessOverlay::parse(PIN).expect("parses");
    let render = render_soc(&SocInputs {
        vendor_svd: &vendor(&config()),
        vendor_path: "chips/test/registers/upstream/vendor.svd",
        overlay: &overlay,
        curated_svd: &curated(),
        curated_typed: &BTreeSet::new(),
        headers: &tree(""),
    })
    .expect("renders");
    assert_eq!(api(&render, "CONFIG"), RegisterApi::Read);
    assert!(reasons(&render, "CONFIG").contains("no pinned header"));
    assert!(!render.rust.contains("pub fn modify_config("));
}

#[test]
fn bits_the_header_declares_but_the_svd_does_not_leave_reads_only() {
    let control = register(
        "CONTROL",
        4,
        "read-write",
        &field("RUN", 0, 1, "read-write"),
    );
    let declared = reg_h(&[(
        "CONTROL",
        4,
        &[
            ("RUN", "R/W", 0, 1, "Runs the timer."),
            ("MODE", "R/W", 1, 2, "The mode."),
        ],
    )]);
    let render = soc(&control, &declared, "").expect("renders");
    assert_eq!(api(&render, "CONTROL"), RegisterApi::Read);
    assert!(reasons(&render, "CONTROL").contains("undeclared bits 0x00000006"));
    assert!(!render.rust.contains("pub fn modify_control("));
    // Bits no header field declares are reserved: kept by every write.
    let reserved = reg_h(&[("CONTROL", 4, &[("RUN", "R/W", 0, 1, "Runs the timer.")])]);
    let render = soc(&control, &reserved, "").expect("renders");
    assert_eq!(api(&render, "CONTROL"), RegisterApi::Full);
    assert!(render.rust.contains("pub fn modify_control("));
}

#[test]
fn a_one_bit_write_trigger_of_the_header_is_written_only_by_its_trigger() {
    let strobe = register(
        "COMMAND",
        8,
        "read-write",
        &(field("GO", 0, 1, "write-only") + &field("STATE", 1, 31, "read-only")),
    );
    let header = |go: &'static str| {
        reg_h(&[(
            "COMMAND",
            8,
            &[
                ("GO", go, 0, 1, "Starts the timer."),
                ("STATE", "RO", 1, 31, "The state."),
            ],
        )])
    };
    let render = soc(&strobe, &header("WO"), "").expect("renders");
    assert_eq!(api(&render, "COMMAND"), RegisterApi::Read);
    assert!(reasons(&render, "COMMAND").contains("write-only field GO"));
    assert!(!render.rust.contains("trigger_command"));
    let trigger = soc(&strobe, &header("WT"), "").expect("renders");
    assert_eq!(api(&trigger, "COMMAND"), RegisterApi::Full);
    assert!(
        trigger.rust.contains("pub fn trigger_command_go("),
        "{}",
        trigger.rust
    );
    assert!(!trigger.rust.contains("pub fn modify_command("));
}

#[test]
fn a_field_the_hardware_also_changes_leaves_its_register_read_only() {
    let alarm = register(
        "ALARM",
        0xc,
        "read-write",
        &(field("ALARM_EN", 0, 1, "read-write") + &field("LIMIT", 1, 31, "read-write")),
    );
    let header = reg_h(&[(
        "ALARM",
        0xc,
        &[
            (
                "ALARM_EN",
                "R/W/SC",
                0,
                1,
                "Cleared by the hardware at the alarm.",
            ),
            ("LIMIT", "R/W", 1, 31, "The limit."),
        ],
    )]);
    let render = soc(&alarm, &header, "").expect("renders");
    assert_eq!(api(&render, "ALARM"), RegisterApi::Read);
    assert!(
        reasons(&render, "ALARM").contains("ALARM_EN is R/W/SC"),
        "{}",
        reasons(&render, "ALARM")
    );
    assert!(!render.rust.contains("pub fn modify_alarm("));
}

/// `SYNC`: two one-bit commands (`R/W/SC`, `R/W/WTC`), a qualifier, a
/// debugging command, a done flag and a plain group id, like
/// CACHE.SYNC_CTRL (`cache_reg.h:1574`) and SPI1.CMD (`spi1_mem_reg.h:16`).
fn sync() -> (String, String) {
    let svd = register(
        "SYNC",
        0x14,
        "read-write",
        &(field("INVALIDATE_ENA", 0, 1, "read-write")
            + &field("CLEAN_ENA", 1, 1, "read-write")
            + &field("ERASE", 2, 1, "read-write")
            + &field("PROBE", 3, 1, "read-write")
            + &field("DONE", 4, 1, "read-only")
            + &field("RGID", 5, 27, "read-write")),
    );
    let header = reg_h(&[(
        "SYNC",
        0x14,
        &[
            (
                "INVALIDATE_ENA",
                "R/W/SC",
                0,
                1,
                "Cleared by hardware when done.",
            ),
            (
                "CLEAN_ENA",
                "R/W/WTC",
                1,
                1,
                "Cleared by hardware when done.",
            ),
            (
                "ERASE",
                "R/W/SC",
                2,
                1,
                "Combined with a command. Cleared by hardware when done.",
            ),
            (
                "PROBE",
                "R/W/SC",
                3,
                1,
                "Cleared by hardware when done. This field is only for internal debugging \
                 purposes. Do not use it in applications.",
            ),
            ("DONE", "RO", 4, 1, "The operation has finished."),
            ("RGID", "R/W", 5, 27, "The group id."),
        ],
    )]);
    (svd, header)
}

/// The `[[commands]]` entry of `SYNC`: `fields` and `qualifiers` as given.
fn commands(header: &str, fields: &str, qualifiers: &str) -> String {
    format!(
        "[[commands]]\nperipheral = \"TIMER\"\nregister = \"SYNC\"\nfields = [{fields}]\n\
         qualifiers = [{qualifiers}]\nsources = [\"esp-idf:{REG}:{}\"]\n",
        line(header, "#define TIMER_SYNC_REG")
    )
}

#[test]
fn a_self_clearing_field_without_a_commands_entry_leaves_its_register_read_only() {
    // A run control (I2S RX_START) and a level the hardware clears (GPIO
    // ENABLE1, STATUS1): that a bit clears itself does not say that writing
    // zero to it is harmless, so no start is inferred from the class.
    let (svd, header) = sync();
    let render = soc(&svd, &header, "").expect("renders");
    assert_eq!(api(&render, "SYNC"), RegisterApi::Read);
    assert!(
        reasons(&render, "SYNC").contains("INVALIDATE_ENA is R/W/SC"),
        "{}",
        reasons(&render, "SYNC")
    );
    assert!(
        reasons(&render, "SYNC").contains("CLEAN_ENA is R/W/WTC"),
        "{}",
        reasons(&render, "SYNC")
    );
    for absent in ["pub fn start_", "pub fn modify_sync(", "_busy("] {
        assert!(!render.rust.contains(absent), "{absent}\n{}", render.rust);
    }
}

#[test]
fn a_commands_entry_makes_a_register_written_only_by_starts_that_wait_for_every_bit() {
    let (svd, header) = sync();
    let entry = commands(&header, "\"INVALIDATE_ENA\", \"CLEAN_ENA\"", "\"ERASE\"");
    let render = soc(&svd, &header, &entry).expect("renders");
    assert_eq!(
        api(&render, "SYNC"),
        RegisterApi::Full,
        "{}",
        reasons(&render, "SYNC")
    );
    for item in [
        "pub fn start_sync_invalidate_ena(",
        "pub fn start_sync_clean_ena(",
        "erase: bool",
        "-> Result<(), oer_mmio::CommandBusy>",
        "pub fn sync_clean_ena_busy(",
        // The debugging command is held: a start waits for it and writes zero.
        "pub const BUSY: u32 = 0x0000000f;",
        "pub const COMMANDS: u32 = 0x00000003;",
        "pub const fn rgid(",
    ] {
        assert!(render.rust.contains(item), "{item}\n{}", render.rust);
    }
    for absent in [
        "pub fn modify_sync(",
        "pub fn set_rgid(",
        "pub fn start_sync_erase(",
        "pub fn start_sync_probe(",
    ] {
        assert!(!render.rust.contains(absent), "{absent}\n{}", render.rust);
    }
    // TIMER1 derives the register but has no entry: it stays read-only.
    assert_eq!(api_of(&render, "TIMER1", "SYNC"), RegisterApi::Read);
}

#[test]
fn a_commands_entry_names_only_self_clearing_bits_an_application_may_start() {
    let (svd, header) = sync();
    for (fields, qualifiers, expected) in [
        ("\"RGID\"", "", "R/W [31:5]"),
        ("\"DONE\"", "", "RO [4:4]"),
        ("\"PROBE\"", "", "internal debugging"),
        ("\"INVALIDATE_ENA\"", "\"PROBE\"", "internal debugging"),
    ] {
        let entry = commands(&header, fields, qualifiers);
        let error = soc(&svd, &header, &entry)
            .err()
            .unwrap_or_else(|| panic!("{fields} {qualifiers} is refused"));
        assert!(error.to_string().contains(expected), "{expected}: {error}");
    }
    // A field the hardware also sets stays a reason the register is only read.
    let set = header.replace("RGID : R/W;", "RGID : R/W/SS;");
    let entry = commands(&set, "\"INVALIDATE_ENA\"", "");
    let error = soc(&svd, &set, &entry)
        .err()
        .expect("RGID is set by the hardware");
    assert!(error.to_string().contains("stays read-only"), "{error}");
}

#[test]
fn a_debugging_field_is_read_only_and_a_debugging_trigger_a_zero_lane() {
    let svd = register(
        "DEBUG",
        0x30,
        "read-write",
        &(field("MODE", 0, 2, "read-write")
            + &field("PROBE", 2, 2, "read-write")
            + &field("KICK", 4, 1, "write-only")),
    );
    let debug = "Only for internal debugging purposes. Do not use it in applications.";
    let fields: &[(&str, &str, u32, u32, &str)] = &[
        ("MODE", "R/W", 0, 2, "The mode."),
        ("PROBE", "R/W", 2, 2, debug),
        ("KICK", "WT", 4, 1, debug),
    ];
    let header = reg_h(&[("DEBUG", 0x30, fields)]);
    let render = soc(&svd, &header, "").expect("renders");
    assert_eq!(api(&render, "DEBUG"), RegisterApi::Full);
    for item in [
        "pub fn set_mode(",
        "pub const fn probe(",
        "pub const ZERO: u32 = 0x00000010;",
    ] {
        assert!(render.rust.contains(item), "{item}\n{}", render.rust);
    }
    for absent in ["pub fn set_probe(", "pub fn trigger_debug_kick("] {
        assert!(!render.rust.contains(absent), "{absent}\n{}", render.rust);
    }
}

#[test]
fn a_data_port_lifts_the_hardware_written_narrowing_and_a_wide_trigger_takes_a_value() {
    let words = register(
        "W%s",
        0x18,
        "read-write",
        &field("BUF", 0, 32, "read-write"),
    )
    .replace(
        "<register>",
        "<register><dim>2</dim><dimIncrement>4</dimIncrement>",
    );
    let tie = register(
        "TIE",
        0x24,
        "write-only",
        &(field("LOW", 0, 4, "write-only") + &field("HIGH", 4, 4, "write-only")),
    );
    let header = reg_h(&[
        ("W0", 0x18, &[("BUF0", "R/W/SS", 0, 32, "data buffer")]),
        ("W1", 0x1c, &[("BUF1", "R/W/SS", 0, 32, "data buffer")]),
        (
            "TIE",
            0x24,
            &[
                ("LOW", "WT", 0, 4, "Ties low."),
                ("HIGH", "WT", 4, 4, "Ties high."),
            ],
        ),
    ]);
    let registers = words.clone() + &tie;
    let render = soc(&registers, &header, "").expect("renders");
    assert_eq!(api(&render, "W%s"), RegisterApi::Read);
    assert_eq!(
        api(&render, "TIE"),
        RegisterApi::Full,
        "{}",
        reasons(&render, "TIE")
    );
    assert!(
        render.rust.contains("pub fn trigger_tie_high("),
        "{}",
        render.rust
    );
    // A word of write triggers alone is written from zero, never read.
    assert!(
        render
            .rust
            .contains("self.0.write::<0x24>(value.get() << 4);"),
        "{}",
        render.rust
    );
    assert!(!render.rust.contains("pub fn read_tie("), "{}", render.rust);
    let port = soc(
        &registers,
        &header,
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"W%s\"\ndata-port = true\n\
         sources = [\"trm:SPI, data buffer\"]\n",
    )
    .expect("renders");
    assert_eq!(
        api(&port, "W%s"),
        RegisterApi::Full,
        "{}",
        reasons(&port, "W%s")
    );
    assert!(port.rust.contains("pub fn write_w("), "{}", port.rust);
    assert!(!port.rust.contains("pub fn modify_w("));
}

/// SOC_ETM.EVT_ST0 (`soc_etm_reg.h:2297`): R/WTC/SS status bits the SVD
/// calls read-write.
#[test]
fn a_write_one_to_clear_status_is_sampled_and_acknowledged_never_modified() {
    let status = register(
        "EVT_ST0",
        0x10,
        "read-write",
        &(field("CH0_RISE_EDGE_ST", 0, 1, "read-write")
            + &field("CH1_RISE_EDGE_ST", 1, 1, "read-write")),
    );
    let header = reg_h(&[(
        "EVT_ST0",
        0x10,
        &[
            ("CH0_RISE_EDGE_ST", "R/WTC/SS", 0, 1, "Channel 0 triggered."),
            ("CH1_RISE_EDGE_ST", "R/WTC/SS", 1, 1, "Channel 1 triggered."),
        ],
    )]);
    let render = soc(&status, &header, "").expect("renders");
    assert_eq!(api(&render, "EVT_ST0"), RegisterApi::Full);
    assert!(
        render.rust.contains("pub fn acknowledge_evt_st0("),
        "{}",
        render.rust
    );
    assert!(!render.rust.contains("pub fn modify_evt_st0("));
    assert!(!render.rust.contains("pub fn write_evt_st0("));
    // Beside a plain field, write-one-to-clear has no acknowledgement.
    let mixed = reg_h(&[(
        "EVT_ST0",
        0x10,
        &[
            ("CH0_RISE_EDGE_ST", "R/WTC/SS", 0, 1, "Channel 0 triggered."),
            ("CH1_RISE_EDGE_ST", "R/W", 1, 1, "Channel 1 enable."),
        ],
    )]);
    let render = soc(&status, &mixed, "").expect("renders");
    assert_eq!(api(&render, "EVT_ST0"), RegisterApi::Read);
    assert!(!render.rust.contains("pub fn modify_evt_st0("));
}

/// GMAC's remote wake-up frame filter port (`gmac_reg.h:916`) and PMT
/// status (`gmac_reg.h:971`).
#[test]
fn a_status_a_read_clears_and_a_sequential_port_have_no_api() {
    let registers = register(
        "WKUPFRM",
        0x28,
        "read-write",
        &field("WKUPFRM_FILTER", 0, 32, "read-write"),
    ) + &register(
        "PMT",
        0x2c,
        "read-write",
        &(field("PWRDWN", 0, 1, "read-write") + &field("MGKPRCVD", 1, 31, "read-write")),
    );
    let header = reg_h(&[
        (
            "WKUPFRM",
            0x28,
            &[(
                "WKUPFRM_FILTER",
                "R/W",
                0,
                32,
                "Eight sequential writes to this address write all filter registers; \
                 eight sequential reads read them.",
            )],
        ),
        (
            "PMT",
            0x2c,
            &[
                ("PWRDWN", "R/W", 0, 1, "Power down."),
                (
                    "MGKPRCVD",
                    "R/W",
                    1,
                    31,
                    "Magic packet received. This bit is cleared by a Read into this register.",
                ),
            ],
        ),
    ]);
    let render = soc(&registers, &header, "").expect("renders");
    for register in ["WKUPFRM", "PMT"] {
        assert_eq!(api(&render, register), RegisterApi::None, "{register}");
    }
    assert!(reasons(&render, "WKUPFRM").contains("in sequence"));
    assert!(reasons(&render, "PMT").contains("cleared by a read"));
    assert!(!render.rust.contains("pub fn read_wkupfrm("));
    assert!(!render.rust.contains("pub fn read_pmt("));
}

/// C5 TIMG T1ALARMLO (`timer_group_struct.h:607-608`: `hw_timer[1]`, then
/// `reserved_024[9]`).
#[test]
fn a_word_the_device_layout_reserves_has_no_api() {
    let registers = config()
        + &register(
            "T1ALARMLO",
            0x40,
            "read-write",
            &field("ALARM_LO", 0, 32, "read-write"),
        )
        + &register(
            "SPARE",
            0x50,
            "read-write",
            &field("VALUE", 0, 32, "read-write"),
        );
    let render = soc(&registers, &config_h(), "").expect("renders");
    assert_eq!(api(&render, "T1ALARMLO"), RegisterApi::None);
    assert!(reasons(&render, "T1ALARMLO").contains("timer_struct.h:3"));
    assert!(!render.rust.contains("t1alarmlo"));
    // A word the header defines no register for, outside reserved words:
    // only read.
    assert_eq!(api(&render, "SPARE"), RegisterApi::Read);
    assert!(reasons(&render, "SPARE").contains("defines no register SPARE"));
}

/// C5 PCR.CTRL_CLK_OUT_EN (`pcr_reg.h:2163-2184`): bit 6 is CLK_480M_OEN,
/// bits 7..9 are "Reserved".
#[test]
fn reserved_lanes_get_no_setter_and_fields_take_their_header_name() {
    let clocks = register(
        "CLK_OUT_EN",
        0x14,
        "read-write",
        &(field("CLK_320M_OEN", 6, 1, "read-write")
            + &field("CLK_ADC_INF_OEN", 7, 1, "read-write")
            + &field("CLK_DAC_CPU_OEN", 8, 1, "read-write")
            + &field("CLK40X_BB_OEN", 9, 1, "read-write")
            + &field("CLK_XTAL_OEN", 10, 1, "read-write")),
    );
    let header = reg_h(&[(
        "CLK_OUT_EN",
        0x14,
        &[
            ("CLK_480M_OEN", "R/W", 6, 1, "Set 1 to enable 480m clock"),
            ("CLK_ADC_INF_OEN", "R/W", 7, 1, "Reserved"),
            ("CLK_DAC_CPU_OEN", "R/W", 8, 1, "Reserved"),
            ("CLK40X_BB_OEN", "R/W", 9, 1, "Reserved"),
            ("CLK_XTAL_OEN", "R/W", 10, 1, "Set 1 to enable xtal clock"),
        ],
    )]);
    let render = soc(&clocks, &header, "").expect("renders");
    assert_eq!(api(&render, "CLK_OUT_EN"), RegisterApi::Full);
    assert!(render.rust.contains("pub fn set_clk_480m_oen("));
    assert!(render.rust.contains("pub fn set_clk_xtal_oen("));
    for absent in [
        "clk_320m_oen",
        "clk_adc_inf_oen",
        "clk_dac_cpu_oen",
        "clk40x_bb_oen",
    ] {
        assert!(!render.rust.contains(absent), "{absent}");
    }
    assert!(render.report.contains(
        "[[renamed]]\nperipheral = \"TIMER\"\nregister = \"CLK_OUT_EN\"\nsvd = \"CLK_320M_OEN\"\n\
         source = \"CLK_480M_OEN\""
    ));
    // An overlay entry citing the line under the SVD's name is refused
    // until it renames the field after its source.
    let cited = line(&header, "TIMER_CLK_480M_OEN :");
    let entry = |rename: &str| {
        format!(
            "[[fields]]\nperipheral = \"TIMER\"\nregister = \"CLK_OUT_EN\"\nfield = \"CLK_320M_OEN\"\n\
             {rename}kind = \"plain\"\nsources = [\"esp-idf:{REG}:{cited}\"]\n"
        )
    };
    let error = soc(&clocks, &header, &entry(""))
        .err()
        .expect("a different name is refused");
    assert!(
        error
            .to_string()
            .contains("is field TIMER_CLK_480M_OEN, not CLK_320M_OEN"),
        "{error}"
    );
    soc(&clocks, &header, &entry("rename = \"CLK_480M_OEN\"\n")).expect("renders");
}

#[test]
fn a_register_the_header_places_elsewhere_is_refused_until_the_overlay_moves_it() {
    let registers = config()
        + &register(
            "STATUS",
            0x8,
            "read-only",
            &field("BUSY", 0, 32, "read-only"),
        );
    let header = reg_h(&[
        (
            "CONFIG",
            0,
            &[
                ("ENABLE", "R/W", 0, 1, "Enables the timer."),
                ("DIVIDER", "R/W", 1, 31, "The clock divider."),
            ],
        ),
        ("ECDSA", 0x8, &[("CFG", "R/W", 0, 32, "The configuration.")]),
        ("STATUS", 0xc, &[("BUSY", "RO", 0, 32, "Busy.")]),
    ]);
    let error = soc(&registers, &header, "")
        .err()
        .expect("a wrong offset is refused");
    assert!(
        error
            .to_string()
            .contains("TIMER.STATUS: the vendor SVD places it at 0x8"),
        "{error}"
    );
    let status = line(&header, "TIMER_STATUS_REG (");
    let ecdsa = line(&header, "TIMER_ECDSA_REG (");
    let corrected = soc(
        &registers,
        &header,
        &format!(
            "[[registers]]\nperipheral = \"TIMER\"\nregister = \"STATUS\"\noffset = 0xc\n\
             sources = [\"esp-idf:{REG}:{status}\"]\n\n\
             [[registers]]\nperipheral = \"TIMER\"\nregister = \"ECDSA\"\nadd = true\n\
             sources = [\"esp-idf:{REG}:{ecdsa}\"]\n"
        ),
    )
    .expect("renders");
    assert_eq!(api(&corrected, "STATUS"), RegisterApi::Read);
    assert_eq!(api(&corrected, "ECDSA"), RegisterApi::Full);
    assert!(corrected.rust.contains("pub fn modify_ecdsa("));
    // A stated offset must be the header's.
    let error = soc(
        &registers,
        &header,
        &format!(
            "[[registers]]\nperipheral = \"TIMER\"\nregister = \"STATUS\"\noffset = 0x10\n\
             sources = [\"esp-idf:{REG}:{status}\"]\n"
        ),
    )
    .err()
    .expect("an offset other than the header's is refused");
    assert!(
        error
            .to_string()
            .contains("TIMER.STATUS: the vendor SVD places it at 0x10"),
        "{error}"
    );
}

#[test]
fn a_port_a_read_may_change_has_no_api_until_the_overlay_names_it_a_fifo() {
    let port = register(
        "FIFO",
        0x10,
        "read-write",
        &field("RXFIFO_RD_BYTE", 0, 32, "read-write"),
    );
    let header = reg_h(&[(
        "FIFO",
        0x10,
        &[("RXFIFO_RD_BYTE", "R/W", 0, 32, "The received byte.")],
    )]);
    let render = soc(&port, &header, "").expect("renders");
    assert_eq!(api(&render, "FIFO"), RegisterApi::None);
    assert!(!render.rust.contains("fifo"));
    let fifo = soc(
        &port,
        &header,
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"FIFO\"\nfifo = true\n\
         sources = [\"trm:UART, FIFO\"]\n",
    )
    .expect("renders");
    assert_eq!(api(&fifo, "FIFO"), RegisterApi::Full);
    assert!(fifo.rust.contains("pop_fifo"), "{}", fifo.rust);
}

#[test]
fn a_raw_status_is_read_unless_its_header_makes_it_write_one_to_clear() {
    let status = register(
        "INT_RAW",
        0x14,
        "read-write",
        &field("DONE", 0, 32, "read-write"),
    );
    let header = |access: &str| {
        reg_h(&[(
            "INT_RAW",
            0x14,
            &[("DONE", access, 0, 32, "The raw done status.")],
        )])
    };
    let render = soc(&status, &header("R/W"), "").expect("renders");
    assert_eq!(api(&render, "INT_RAW"), RegisterApi::Read);
    assert!(reasons(&render, "INT_RAW").contains("raw interrupt status"));
    let clear = soc(&status, &header("R/WTC/SS"), "").expect("renders");
    assert_eq!(api(&clear, "INT_RAW"), RegisterApi::Full);
    assert!(clear.rust.contains("pub fn acknowledge_int_raw("));
    assert!(!clear.rust.contains("pub fn modify_int_raw("));
}

#[test]
fn a_word_the_curated_model_describes_is_not_rendered_again() {
    let radio = register(
        "RADIO",
        0x20,
        "read-write",
        &field("VALUE", 0, 32, "read-write"),
    );
    let header = config_h() + &reg_h(&[("RADIO", 0x20, &[("VALUE", "R/W", 0, 32, "A value.")])]);
    let render = soc(&(config() + &radio), &header, "").expect("renders");
    assert_eq!(api(&render, "RADIO"), RegisterApi::None);
    assert!(reasons(&render, "RADIO").contains("TIMER_RADIO"));
    // The derived block's word at its own address is not the curated one.
    assert!(
        render
            .verdicts
            .iter()
            .any(|verdict| verdict.peripheral == "TIMER1"
                && verdict.register == "RADIO"
                && verdict.api == RegisterApi::Full)
    );
}

#[test]
fn a_derived_peripheral_is_its_own_block_with_its_interrupt() {
    let render = soc(&config(), &config_h(), "").expect("renders");
    assert_eq!(render.peripherals, ["TIMER", "TIMER1"]);
    assert!(render.rust.contains("pub mod timer1 {"));
    assert!(render.rust.contains("pub const BASE: usize = 0x60001000;"));
    assert!(render.rust.contains(
        "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for Timer1<P> {\nconst SOURCE: u16 = 10;"
    ));
    // One partition holds every block; the root claims them at once.
    assert!(render.rust.contains("pub struct SocPeripherals {"));
    assert!(render.rust.contains("pub type Claims ="));
    // Vendor descriptions are single lines: no list item, no link.
    assert!(
        render.rust.contains("Timer \\[0\\] - one"),
        "{}",
        render.rust
    );
}

#[test]
fn the_interrupt_enum_has_the_svd_and_overlay_sources_but_no_source_count() {
    let from_cpu = line(&tree("").0[INTERRUPTS], "ETS_CPU_INTR_FROM_CPU_0_SOURCE");
    let render = soc(
        &config(),
        &config_h(),
        &format!(
            "[[interrupts]]\nname = \"FROM_CPU_INTR0\"\nsource = 84\n\
             sources = [\"esp-idf:{INTERRUPTS}:{from_cpu}\"]\n"
        ),
    )
    .expect("renders");
    for item in [
        "pub enum Interrupt {",
        "TIMER0 = 9,",
        "TIMER1 = 10,",
        &format!("/// SOURCE: esp-idf:{INTERRUPTS}:{from_cpu}.\nFROM_CPU_INTR0 = 84,"),
        "84 => Some(Self::FROM_CPU_INTR0),",
    ] {
        assert!(render.rust.contains(item), "{item}\n{}", render.rust);
    }
    let error = soc(
        &config(),
        &config_h(),
        "[[interrupts]]\nname = \"TIMER0\"\nsource = 9\nsources = [\"trm:Interrupt Matrix\"]\n",
    )
    .err()
    .expect("a listed source is refused");
    assert!(
        error.to_string().contains("is in the vendor SVD"),
        "{error}"
    );
    // ETS_MAX_INTR_SOURCE counts the sources; it is none.
    let count = line(&tree("").0[INTERRUPTS], "ETS_MAX_INTR_SOURCE");
    let error = soc(
        &config(),
        &config_h(),
        &format!(
            "[[interrupts]]\nname = \"MAX\"\nsource = 85\n\
             sources = [\"esp-idf:{INTERRUPTS}:{count}\"]\n"
        ),
    )
    .err()
    .expect("the source count is refused");
    assert!(error.to_string().contains("not a source"), "{error}");
}

#[test]
fn every_overlay_entry_names_vendor_geometry_and_cites_a_source() {
    let missing = soc(
        &config(),
        &config_h(),
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"ABSENT\"\nreserved = 1\n\
         sources = [\"trm:Timer\"]\n",
    )
    .err()
    .expect("an absent register is refused");
    assert!(
        missing.to_string().contains("not in the vendor SVD"),
        "{missing}"
    );
    for source in ["\"\"", "\"esp-idf:path\"", "\"vendor says so\""] {
        let error = soc(
            &config(),
            &config_h(),
            &format!(
                "[[fields]]\nperipheral = \"TIMER\"\nregister = \"CONFIG\"\nfield = \"ENABLE\"\n\
                 kind = \"plain\"\nsources = [{source}]\n"
            ),
        )
        .err()
        .expect("an uncited entry is refused");
        assert!(error.to_string().contains("cites"), "{error}");
    }
    let overlap = soc(
        &config(),
        &config_h(),
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"CONFIG\"\nreserved = 1\n\
         sources = [\"trm:Timer\"]\n",
    )
    .err()
    .expect("reserving a declared bit is refused");
    assert!(overlap.to_string().contains("reserves bits"), "{overlap}");
    // A cited line must be the register's.
    let error = soc(
        &config(),
        &config_h(),
        &format!(
            "[[fields]]\nperipheral = \"TIMER\"\nregister = \"CONFIG\"\nfield = \"ENABLE\"\n\
             kind = \"plain\"\nsources = [\"esp-idf:{REG}:1\"]\n"
        ),
    )
    .err()
    .expect("a line that is no field is refused");
    assert!(error.to_string().contains("is not a field line"), "{error}");
}

#[test]
fn a_peripheral_stated_without_api_renders_no_register_and_no_method() {
    let overlay = "\n[[no-api]]\nperipheral = \"TIMER\"\nreason = \"mixed semantics described only in text\"\nsources = [\"trm:test\"]\n\
         [[no-api]]\nperipheral = \"TIMER1\"\nreason = \"the same block\"\nsources = [\"trm:test\"]\n";
    let render = soc(&config(), &config_h(), overlay).expect("renders");
    assert_eq!(api(&render, "CONFIG"), RegisterApi::None);
    assert!(reasons(&render, "CONFIG").contains("mixed semantics described only in text"));
    assert!(!render.rust.contains("pub fn modify_config("));
    assert!(!render.rust.contains("pub fn read_config("));
}

#[test]
fn a_no_api_peripheral_the_svd_lacks_is_an_error() {
    let overlay =
        "\n[[no-api]]\nperipheral = \"GHOST\"\nreason = \"none\"\nsources = [\"trm:test\"]\n";
    assert!(soc(&config(), &config_h(), overlay).is_err());
}
