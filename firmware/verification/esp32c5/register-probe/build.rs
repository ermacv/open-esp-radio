//! Link the binary with esp-hal's linker script, wherever Cargo runs from.

fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
}
