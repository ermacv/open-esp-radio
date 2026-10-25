use super::*;

#[test]
fn a_one_bit_literal_is_the_bit_by_shift_by_macro_or_by_value() {
    for literal in [
        "(1 << 31)",
        "1<<31",
        "(1U << 31)",
        "BIT(31)",
        "0x80000000",
        "(0x80000000U)",
    ] {
        assert!(bit_literal(literal, 31), "{literal}");
    }
    for other in ["(1 << 30)", "(2 << 31)", "BIT(30)", "x << 31", "0x40000000"] {
        assert!(!bit_literal(other, 31), "{other}");
    }
}

#[test]
fn an_assignment_names_its_target_and_a_comparison_is_none() {
    assert_eq!(
        assignment("    uint32_t bits = (div << A_S); // the word"),
        Some(("bits", "(div << A_S)"))
    );
    assert_eq!(assignment("    word |= 1;"), None, "a compound assignment");
    assert_eq!(assignment("    if (a == b) {"), None);
    assert_eq!(assignment("    a <= b;"), None);
}

#[test]
fn one_function_holds_the_lines_its_braces_hold_and_comments_do_not_count() {
    let text = "void f(void)\n{\n    a = 1; /* } */\n    // }\n    return a;\n}\nvoid g(void)\n{\n    b = 1;\n}\n";
    assert!(same_function(text, 3, 5));
    assert!(!same_function(text, 3, 9), "across the end of f");
    assert!(
        !same_function(text, 1, 3),
        "the signature is outside the body"
    );
    assert!(!same_function(text, 5, 99), "past the file's end");
}
