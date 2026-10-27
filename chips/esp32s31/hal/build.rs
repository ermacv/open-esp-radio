//! Generate the controller geometry from the same facts as runtime vectors.
use std::fmt::Write;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let (facts, files) = oer_espressif_facts::generate::tracked("esp32s31")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    let geometry = oer_espressif_facts::generate::Vectors::of(&facts)?;
    std::fs::write(out.join("interrupts.rs"), geometry.rust()?)?;
    std::fs::write(out.join("pending.rs"), pending_banks(geometry.harts)?)?;
    Ok(())
}

/// Every pending owner and selection arm follows the facts' complete geometry.
fn pending_banks(harts: u32) -> Result<String, std::fmt::Error> {
    let mut rust = String::from("struct PendingBanks<\n");
    for hart in 0..harts {
        writeln!(
            rust,
            "P{hart}: Placement = Fixed<{{ instances::interrupt_core{hart}::BASE }}>,"
        )?;
    }
    rust.push_str("> {\n");
    for hart in 0..harts {
        writeln!(
            rust,
            "hart{hart}: oer_esp32s31_pac::interrupt_core::InterruptCore{hart}Status<P{hart}>,"
        )?;
    }
    rust.push_str("}\nimpl<");
    for hart in 0..harts {
        write!(rust, "P{hart}: Placement,")?;
    }
    rust.push_str("> PendingBanks<");
    for hart in 0..harts {
        write!(rust, "P{hart},")?;
    }
    rust.push_str("> {\nfn is_pending(&self, hart: crate::platform::Hart, source: u16) -> bool {\nmatch hart {\n");
    for hart in 0..harts {
        writeln!(
            rust,
            "crate::platform::Hart::H{hart} => pending_in(&self.hart{hart}, source),"
        )?;
    }
    rust.push_str("}\n}\n}\n");
    for hart in 0..harts {
        writeln!(rust, "pending_status!(InterruptCore{hart}Status);")?;
    }
    rust.push_str("macro_rules! take_pending { ($parts:ident) => { PendingBanks {\n");
    for hart in 0..harts {
        writeln!(rust, "hart{hart}: $parts.interrupt_core{hart}_status,")?;
    }
    rust.push_str("} }; }\n");
    rust.push_str("#[cfg(test)]\nmacro_rules! host_pending { ($words:expr) => {{\nlet [");
    for hart in 0..harts {
        write!(rust, "hart{hart},")?;
    }
    rust.push_str("] = $words;\n$crate::platform::interrupt_controller::PendingBanks {\n");
    for hart in 0..harts {
        writeln!(
            rust,
            "hart{hart}: oer_esp32s31_pac::interrupt_core::InterruptCore{hart}Status::host(hart{hart}).expect(\"host pending bank\"),"
        )?;
    }
    rust.push_str("}\n}}; }\n");
    Ok(rust)
}
