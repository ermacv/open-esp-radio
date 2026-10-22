//! The driver loads the exact compiler libraries of its build toolchain.
#![forbid(unsafe_code)]

fn main() {
    let rustc = std::env::var_os("RUSTC").expect("Cargo supplies its compiler");
    let output = std::process::Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .expect("the pinned compiler runs");
    assert!(output.status.success(), "the pinned compiler has a sysroot");
    let sysroot = String::from_utf8(output.stdout).expect("the sysroot is UTF-8");
    println!("cargo:rustc-link-search=native={}/lib", sysroot.trim());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}/lib", sysroot.trim());
}
