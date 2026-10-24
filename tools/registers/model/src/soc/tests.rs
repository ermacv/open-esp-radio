use std::collections::BTreeSet;

use super::{RegisterApi, SocInputs, SocRender, StrictnessOverlay, render_soc};

const PIN: &str = "schema = 1\nesp-idf = \"4d59230ddff16327812782151ef0afef202dc6d7\"\n\
                   esp-hal = \"1f646c5af4f4ca2774f493783581237fea2379dc\"\n";

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

fn soc(registers: &str, overlay: &str) -> crate::Result<SocRender> {
    let overlay = StrictnessOverlay::parse(&format!("{PIN}{overlay}"))?;
    render_soc(&SocInputs {
        vendor_svd: &vendor(registers),
        vendor_path: "chips/test/registers/upstream/vendor.svd",
        overlay: &overlay,
        curated_svd: &curated(),
        curated_typed: &BTreeSet::new(),
    })
}

fn api(render: &SocRender, register: &str) -> RegisterApi {
    render
        .verdicts
        .iter()
        .find(|verdict| verdict.peripheral == "TIMER" && verdict.register == register)
        .unwrap_or_else(|| panic!("{register} has a verdict"))
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

/// A register whose every bit is a declared plain field keeps reads and
/// read-modify-writes.
fn config() -> String {
    register(
        "CONFIG",
        0,
        "read-write",
        &(field("ENABLE", 0, 1, "read-write") + &field("DIVIDER", 1, 31, "read-write")),
    )
}

#[test]
fn a_fully_declared_plain_register_keeps_its_field_api() {
    let render = soc(&config(), "").expect("renders");
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
fn undeclared_bits_leave_reads_only_until_the_overlay_reserves_them() {
    let control = register(
        "CONTROL",
        4,
        "read-write",
        &field("RUN", 0, 1, "read-write"),
    );
    let render = soc(&control, "").expect("renders");
    assert_eq!(api(&render, "CONTROL"), RegisterApi::Read);
    assert!(reasons(&render, "CONTROL").contains("undeclared bits 0xfffffffe"));
    assert!(render.rust.contains("pub fn read_control("));
    assert!(!render.rust.contains("pub fn modify_control("));
    assert!(!render.rust.contains("pub fn write_control("));
    assert!(
        render
            .report
            .contains("register = \"CONTROL\"\napi = \"read\"")
    );
    let reserved = soc(
        &control,
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"CONTROL\"\nreserved = 0xfffffffe\n\
         sources = [\"esp-idf:components/soc/test/timer_struct.h:12\"]\n",
    )
    .expect("renders");
    assert_eq!(api(&reserved, "CONTROL"), RegisterApi::Full);
    assert!(reserved.rust.contains("pub fn modify_control("));
}

#[test]
fn a_write_only_field_is_a_trigger_candidate_without_an_api() {
    let strobe = register(
        "COMMAND",
        8,
        "read-write",
        &(field("GO", 0, 1, "write-only") + &field("STATE", 1, 31, "read-only")),
    );
    let render = soc(&strobe, "").expect("renders");
    assert_eq!(api(&render, "COMMAND"), RegisterApi::Read);
    assert!(reasons(&render, "COMMAND").contains("write-only field GO"));
    assert!(!render.rust.contains("trigger_command"));
    assert!(!render.rust.contains("pub fn modify_command("));
    // Only the overlay's write trigger writes it, once.
    let declared = soc(
        &strobe,
        "[[fields]]\nperipheral = \"TIMER\"\nregister = \"COMMAND\"\nfield = \"GO\"\n\
         kind = \"write-trigger\"\naccess = \"read-write\"\n\
         sources = [\"trm:Timer Group, 12.4\"]\n",
    )
    .expect("renders");
    assert_eq!(api(&declared, "COMMAND"), RegisterApi::Full);
    assert!(
        declared.rust.contains("pub fn trigger_command_go("),
        "{}",
        declared.rust
    );
}

#[test]
fn a_trigger_name_on_a_read_write_field_narrows_its_register() {
    let update = register(
        "UPDATE",
        0xc,
        "read-write",
        &(field("TIMER_UPDATE", 0, 1, "read-write") + &field("SPARE", 1, 31, "read-write")),
    );
    let render = soc(&update, "").expect("renders");
    assert_eq!(api(&render, "UPDATE"), RegisterApi::Read);
    assert!(reasons(&render, "UPDATE").contains("TIMER_UPDATE: a write-trigger candidate"));
    let plain = soc(
        &update,
        "[[fields]]\nperipheral = \"TIMER\"\nregister = \"UPDATE\"\nfield = \"TIMER_UPDATE\"\n\
         kind = \"plain\"\nsources = [\"esp-idf:components/soc/test/timer_struct.h:40\"]\n",
    )
    .expect("renders");
    assert_eq!(api(&plain, "UPDATE"), RegisterApi::Full);
}

#[test]
fn a_port_a_read_may_change_has_no_api_until_the_overlay_names_it_a_fifo() {
    let port = register(
        "FIFO",
        0x10,
        "read-write",
        &field("RXFIFO_RD_BYTE", 0, 32, "read-write"),
    );
    let render = soc(&port, "").expect("renders");
    assert_eq!(api(&render, "FIFO"), RegisterApi::None);
    assert!(!render.rust.contains("fifo"));
    let fifo = soc(
        &port,
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"FIFO\"\nfifo = true\n\
         sources = [\"esp-idf:components/hal/test/uart_ll.h:300\"]\n",
    )
    .expect("renders");
    assert_eq!(api(&fifo, "FIFO"), RegisterApi::Full);
    assert!(fifo.rust.contains("pop_fifo"), "{}", fifo.rust);
}

#[test]
fn a_raw_status_is_read_and_a_cited_one_to_clear_register_acknowledges() {
    let status = register(
        "INT_RAW",
        0x14,
        "read-write",
        &field("DONE", 0, 32, "read-write"),
    );
    let render = soc(&status, "").expect("renders");
    assert_eq!(api(&render, "INT_RAW"), RegisterApi::Read);
    let clear = soc(
        &status,
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"INT_RAW\"\nwrite-one-to-clear = true\n\
         raw-status-writable = true\nsources = [\"esp-idf:components/soc/test/timer_struct.h:80\"]\n",
    )
    .expect("renders");
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
    let render = soc(&(config() + &radio), "").expect("renders");
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
    let render = soc(&config(), "").expect("renders");
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
fn the_interrupt_enum_has_the_svd_and_overlay_sources() {
    let render = soc(
        &config(),
        "[[interrupts]]\nname = \"FROM_CPU_INTR0\"\nsource = 84\n\
         sources = [\"esp-idf:components/soc/test/include/soc/interrupts.h:103\"]\n",
    )
    .expect("renders");
    for item in [
        "pub enum Interrupt {",
        "TIMER0 = 9,",
        "TIMER1 = 10,",
        "/// SOURCE: esp-idf:components/soc/test/include/soc/interrupts.h:103.\nFROM_CPU_INTR0 = 84,",
        "84 => Some(Self::FROM_CPU_INTR0),",
    ] {
        assert!(render.rust.contains(item), "{item}\n{}", render.rust);
    }
    let error = soc(
        &config(),
        "[[interrupts]]\nname = \"TIMER0\"\nsource = 9\nsources = [\"trm:Interrupt Matrix\"]\n",
    )
    .err()
    .expect("a listed source is refused");
    assert!(
        error.to_string().contains("is in the vendor SVD"),
        "{error}"
    );
}

#[test]
fn every_overlay_entry_names_vendor_geometry_and_cites_a_source() {
    let missing = soc(
        &config(),
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
        "[[registers]]\nperipheral = \"TIMER\"\nregister = \"CONFIG\"\nreserved = 1\n\
         sources = [\"trm:Timer\"]\n",
    )
    .err()
    .expect("reserving a declared bit is refused");
    assert!(overlap.to_string().contains("reserves bits"), "{overlap}");
}
