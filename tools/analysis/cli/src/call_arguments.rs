//! Human rendering of call argument values.
use crate::listing::signed_hex;
use oer_analysis_blobray_application::call_arguments::ArgumentValue;

/// `0x6b`, `arg0`, `phy_param+0xef`, `one-of{0x2 | 0x40}`, `?`.
pub fn human(value: &ArgumentValue) -> String {
    match value {
        ArgumentValue::Constant { value } => format!("{value:#x}"),
        ArgumentValue::ImageAddress { address } => format!("{address:#x}"),
        ArgumentValue::Symbol { name, addend } => {
            format!("{}{}", String::from_utf8_lossy(name), signed_hex(*addend))
        }
        ArgumentValue::Section { section, offset } => {
            format!("section{section}{}", signed_hex(*offset))
        }
        ArgumentValue::EntryStack { offset } => format!("sp{}", signed_hex(*offset)),
        ArgumentValue::Alternatives { values } => format!(
            "one-of{{{}}}",
            values.iter().map(human).collect::<Vec<_>>().join(" | ")
        ),
        ArgumentValue::EntryArgument { index } => format!("arg{index}"),
        ArgumentValue::EntryRegister { register } => {
            format!(
                "entry {}",
                oer_analysis_riscv_lift::register_name(*register)
            )
        }
        ArgumentValue::Unknown => String::from("?"),
    }
}

#[cfg(test)]
mod tests;
