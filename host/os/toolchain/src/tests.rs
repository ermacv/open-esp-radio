use super::*;

fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into())
    }
}

fn no_sysroot(_: &OsStr) -> Result<PathBuf> {
    Err("no sysroot in this test".into())
}

#[test]
fn a_non_empty_override_names_the_program() {
    for tool in Tool::ALL {
        let program = program_in(
            tool,
            environment(&[(tool.variable(), "/opt/x")]),
            no_sysroot,
        );
        assert_eq!(program.unwrap(), OsString::from("/opt/x"));
    }
}

#[test]
fn without_an_override_path_tools_are_their_names() {
    let env = environment(&[("CARGO", ""), ("ESPFLASH", "")]);
    for tool in [Tool::Cargo, Tool::Rustc, Tool::Espflash] {
        assert_eq!(
            program_in(tool, &env, no_sysroot).unwrap(),
            OsString::from(tool.name())
        );
    }
}

#[test]
fn llvm_tools_come_from_the_sysroot_of_the_selected_rustc() {
    let directory = tempfile::tempdir().unwrap();
    let nm = directory
        .path()
        .join(format!("llvm-nm{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&nm, "").unwrap();
    let seen = std::cell::RefCell::new(None);
    let program = program_in(
        Tool::LlvmNm,
        environment(&[("LLVM_NM", ""), ("RUSTC", "/pinned/rustc")]),
        |rustc| {
            *seen.borrow_mut() = Some(rustc.to_owned());
            Ok(directory.path().to_owned())
        },
    )
    .unwrap();
    assert_eq!(program, nm.into_os_string());
    assert_eq!(seen.into_inner(), Some(OsString::from("/pinned/rustc")));
    // A missing component is an error naming the override, not PATH's tool.
    let error = program_in(Tool::LlvmObjdump, environment(&[]), |_| {
        Ok(directory.path().to_owned())
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("llvm-tools") && error.contains("LLVM_OBJDUMP"),
        "{error}"
    );
}

#[test]
fn the_active_toolchain_has_every_recorded_llvm_tool() {
    for tool in [Tool::LlvmNm, Tool::LlvmObjcopy, Tool::LlvmObjdump] {
        let program = program_in(tool, environment(&[]), sysroot_bin).unwrap();
        assert!(
            Path::new(&program).is_file(),
            "{}",
            Path::new(&program).display()
        );
    }
}

#[test]
fn every_recorded_tool_is_listed_once_with_llvm_objdump() {
    let names: Vec<_> = Tool::ALL.into_iter().map(Tool::name).collect();
    assert_eq!(
        names,
        [
            "rustc",
            "cargo",
            "llvm-objcopy",
            "llvm-nm",
            "llvm-objdump",
            "espflash"
        ]
    );
}

#[test]
fn the_host_triple_is_the_host_line_of_rustc_verbose_version() {
    let verbose = "rustc 1.95.0\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.95.0\n";
    assert_eq!(super::host_of(verbose).unwrap(), "x86_64-unknown-linux-gnu");
    assert!(super::host_of("rustc 1.95.0\n").is_err());
}

#[test]
fn the_pinned_toolchain_ships_the_lld_that_links_rv32_images() {
    let lld = lld().unwrap();
    let name = format!("ld.lld{}", std::env::consts::EXE_SUFFIX);
    assert!(
        lld.ends_with(Path::new("gcc-ld").join(name)),
        "{}",
        lld.display()
    );
    assert!(lld.is_file());
}
