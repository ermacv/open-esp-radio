use super::{Class, class, read_clears, reserved_description, sequential_port};
use crate::soc_overlay::{parse_layout_reserved, parse_reg_header, same_field};

#[test]
fn access_classes_map_to_what_a_field_may_do() {
    for (access, expected) in [
        ("R/W", Class::Plain),
        ("RO", Class::ReadOnly),
        ("HRO", Class::ReadOnly),
        ("RO/SS", Class::ReadOnly),
        ("WT", Class::Trigger),
        ("WO", Class::WriteOnly),
        ("R/WTC/SS", Class::OneToClear),
        ("R/SS/WTC", Class::OneToClear),
        ("R/W1C", Class::OneToClear),
        ("R/W/SC", Class::SelfClearing),
        ("R/W/WTC", Class::SelfClearing),
        ("R/W/SS", Class::HardwareWritable),
        ("R/W/SS/SC", Class::HardwareWritable),
        ("R/W1S", Class::SetOnly),
        ("R/WTC/WTS", Class::SetOnly),
        ("WS", Class::SetOnly),
        ("R/W1 ", Class::SetOnly),
        ("XYZ", Class::Unknown),
    ] {
        assert_eq!(class(access), expected, "{access}");
    }
}

#[test]
fn descriptions_name_reserved_lanes_read_clears_and_sequential_ports() {
    for reserved in [
        "Reserved",
        "reserved.",
        "RESERVED",
        "Reserved, it was created",
    ] {
        assert!(reserved_description(reserved), "{reserved}");
    }
    assert!(!reserved_description("Reserves the bus."));
    assert!(!reserved_description(
        "This bit is reserved in some configurations"
    ));
    for clears in [
        "This bit is cleared by a Read into this register.",
        "This bit is cleared when read",
        "This bit is cleared on reading the register",
        "set when R_SS_RC any condition holds",
    ] {
        assert!(read_clears(clears), "{clears}");
    }
    assert!(!read_clears(
        "write 1 to clear the interrupt for read operations"
    ));
    assert!(sequential_port(
        "Eight sequential writes to this address write all registers"
    ));
    assert!(!sequential_port("The sequence number."));
}

#[test]
fn a_field_name_is_the_header_s_words_in_order() {
    assert!(same_field("PCR_CLK_480M_OEN", "CLK_480M_OEN"));
    assert!(same_field("SPI1_MEM_C_SYNC_RESET", "SPI_MEM_SYNC_RESET"));
    assert!(same_field(
        "AHB_DMA_IN_DONE_CH0_INT_RAW",
        "IN_DONE_CH_INT_RAW"
    ));
    assert!(same_field("PCR_32K_SEL", "_32K_SEL"));
    assert!(!same_field("PCR_CLK_480M_OEN", "CLK_320M_OEN"));
    assert!(!same_field("ECDSA_SOFTWARE_SET_Z", "DETERMINISTIC_K"));
}

#[test]
fn a_register_header_gives_each_field_its_class_and_description() {
    let text = "#define TIMER_CONFIG_REG (DR_REG_TIMER_BASE + 0x8)\n\
                /** TIMER_EN : R/W/SC; bitpos: [0]; default: 0;\n\
                \x20*  Enables the timer;\n\
                \x20*  the hardware clears it.\n\
                \x20*/\n\
                #define TIMER_EN (BIT(0))\n\
                /** TIMER_SPARE : R/W; bitpos: [7:1]; default: 0; */\n";
    let registers = parse_reg_header(text);
    let [register] = registers.as_slice() else {
        panic!("one register: {registers:?}");
    };
    assert_eq!(
        (register.name.as_str(), register.offset, register.line),
        ("TIMER_CONFIG", 8, 1)
    );
    let [enable, spare] = register.fields.as_slice() else {
        panic!("two fields");
    };
    assert_eq!(enable.access, "R/W/SC");
    assert_eq!(
        enable.description,
        "Enables the timer; the hardware clears it."
    );
    assert_eq!(
        (spare.low, spare.width(), spare.description.as_str()),
        (1, 7, "")
    );
}

#[test]
fn the_device_layout_names_its_reserved_words() {
    let text = "typedef struct {\n    uint32_t reserved_000[3];\n} timg_hwtimer_reg_t;\n\
                typedef struct timg_dev_t {\n    volatile timg_hwtimer_reg_t hw_timer[1];\n\
                \x20   uint32_t reserved_024[9];\n    uint32_t reserved_0f0;\n} timg_dev_t;\n";
    let words = parse_layout_reserved(text);
    let found = words
        .iter()
        .map(|words| (words.offset, words.words, words.line))
        .collect::<Vec<_>>();
    assert_eq!(found, [(0x24, 9, 6), (0xf0, 1, 7)]);
    // Two device layouts in one file name no reserved word.
    assert!(parse_layout_reserved(&format!("{text}{text}")).is_empty());
}
