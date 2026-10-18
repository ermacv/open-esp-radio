//! In-process linking of captured objects named by content, with real ELF
//! linkers and adversarial test linkers.
#![cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../../cli/tests/support/mod.rs"]
mod support;
use blobray_application::in_process::{Executable, Limits};
use blobray_application::linking::{LinkedImage, link, propose_companions};
use blobray_domain::*;
use blobray_linker::ElfLinker;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_riscv_model::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn linker() -> PathBuf {
    std::env::var_os("BLOBRAY_TEST_LLD")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/usr/bin/ld.lld".into())
}

/// System GNU ld names whose builds include the `elf32lriscv` emulation; the
/// adapter selects that emulation explicitly and probes capabilities itself.
const GNU_LINKER_NAMES: [&str; 6] = [
    "riscv-none-elf-ld",
    "riscv32-unknown-elf-ld",
    "riscv32-esp-elf-ld",
    "riscv64-unknown-elf-ld",
    "riscv64-elf-ld",
    "riscv64-linux-gnu-ld",
];

fn gnu_linker() -> PathBuf {
    if let Some(path) = std::env::var_os("BLOBRAY_TEST_GNU_LD").map(PathBuf::from) {
        assert!(
            path.is_file(),
            "GNU RV32 linker is mandatory: BLOBRAY_TEST_GNU_LD={} is not a file",
            path.display()
        );
        return path;
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    GNU_LINKER_NAMES
        .iter()
        .find_map(|name| {
            std::env::split_paths(&search)
                .map(|directory| directory.join(name))
                .find(|path| path.is_file())
        })
        .unwrap_or_else(|| {
            panic!(
                "GNU RV32 linker is mandatory: none of {GNU_LINKER_NAMES:?} is on PATH; install RISC-V GNU binutils 2.47+ or set BLOBRAY_TEST_GNU_LD"
            )
        })
}

fn symbol(name: &[u8], section: SymbolSection, size: u64, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.into(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}

/// `entry` calling `helper` and storing the address of `value`, or the
/// object defining `helper` and the data `value`.
fn object(entry: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    if entry {
        let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
        obj.append_section_data(
            text,
            &[
                0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x67, 0x80, 0, 0, 0, 0, 0, 0,
            ],
            4,
        );
        obj.add_symbol(symbol(
            b"entry",
            SymbolSection::Section(text),
            16,
            SymbolKind::Text,
        ));
        for (offset, name, r_type) in [
            (0, &b"helper"[..], object::elf::R_RISCV_CALL),
            (12, b"value", object::elf::R_RISCV_32),
        ] {
            let target = obj.add_symbol(symbol(
                name,
                SymbolSection::Undefined,
                0,
                SymbolKind::Unknown,
            ));
            obj.add_relocation(
                text,
                Relocation {
                    offset,
                    symbol: target,
                    addend: 0,
                    flags: RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
        }
    } else {
        let text = obj.add_section(Vec::new(), b".text.helper".to_vec(), SectionKind::Text);
        obj.append_section_data(text, &[0x67, 0x80, 0, 0], 4);
        obj.add_symbol(symbol(
            b"helper",
            SymbolSection::Section(text),
            4,
            SymbolKind::Text,
        ));
        let data = obj.add_section(Vec::new(), b".data".to_vec(), SectionKind::Data);
        obj.append_section_data(data, &[0x78, 0x56, 0x34, 0x12], 4);
        obj.add_symbol(symbol(
            b"value",
            SymbolSection::Section(data),
            4,
            SymbolKind::Data,
        ));
    }
    obj.write().unwrap()
}

/// A function `name` that stores the address of `dependency`, when given.
fn dependency_object(name: &[u8], dependency: Option<&[u8]>) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let mut section_name = b".text.".to_vec();
    section_name.extend_from_slice(name);
    let section = obj.add_section(Vec::new(), section_name, SectionKind::Text);
    obj.append_section_data(section, &[0x67, 0x80, 0, 0, 0, 0, 0, 0], 4);
    obj.add_symbol(symbol(
        name,
        SymbolSection::Section(section),
        8,
        SymbolKind::Text,
    ));
    if let Some(dependency) = dependency {
        let target = obj.add_symbol(symbol(
            dependency,
            SymbolSection::Undefined,
            0,
            SymbolKind::Unknown,
        ));
        obj.add_relocation(
            section,
            Relocation {
                offset: 4,
                symbol: target,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}

fn layout() -> ImageLayout {
    ImageLayout {
        code: ImageRegion {
            start: 0x10000000,
            length: 65536,
        },
        data: ImageRegion {
            start: 0x20000000,
            length: 65536,
        },
    }
}

/// The exact symbol `name` of object `member` of `executable`.
fn defined(executable: &Executable, member: usize, name: &[u8]) -> SymbolId {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let inventory = blobray_application::captured::inventory(
        executable,
        &memory,
        &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30)),
    )
    .unwrap();
    inventory.objects[member]
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(name) && s.raw_section != 0)
        .unwrap()
        .id
        .clone()
}

/// Captured inputs in link order and a request whose entry is `entry` of
/// object `member` of input `entry_input`.
struct Fixture {
    dir: tempfile::TempDir,
    inputs: Vec<Executable>,
    request: LinkRequest,
}

fn fixture(inputs: Vec<Vec<u8>>, entry_input: usize, member: usize) -> Fixture {
    let inputs: Vec<_> = inputs.into_iter().map(Executable::new).collect();
    let entry = defined(&inputs[entry_input], member, b"entry");
    Fixture {
        dir: tempfile::tempdir().unwrap(),
        request: LinkRequest {
            companions: vec![],
            inputs: inputs.iter().map(|e| e.id().clone()).collect(),
            entry,
            roots: vec![],
            layout: layout(),
            absent: vec![],
        },
        inputs,
    }
}

/// `entry.a` holding the entry object and `helper.a` defining its references.
fn closed() -> Fixture {
    fixture(
        vec![
            support::archive(&[(b"entry.o", &object(true))], false),
            support::archive(&[(b"helper.o", &object(false))], false),
        ],
        0,
        0,
    )
}

fn link_with(f: &Fixture, tool: &Path, memory: u64) -> Result<LinkedImage> {
    link(
        &f.request,
        &f.inputs,
        tool,
        &ElfLinker,
        f.dir.path(),
        &WorkingMemory::new(memory).unwrap(),
        &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30)),
    )
}

fn linked(f: &Fixture) -> Result<LinkedImage> {
    link_with(f, &linker(), 32 * 1024 * 1024)
}

/// A linker that runs `body`, except for version queries and the capability
/// probe, which the real LLD answers.
fn test_linker(directory: &Path, body: &str) -> PathBuf {
    wrapper(directory, "test-linker", &linker(), body)
}

fn gnu_test_linker(directory: &Path, body: &str) -> PathBuf {
    wrapper(
        directory,
        "gnu-test-linker",
        &gnu_linker().canonicalize().unwrap(),
        body,
    )
}

fn wrapper(directory: &Path, name: &str, real: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = directory.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then exec '{}' --version; fi\ncase \"$PWD\" in */probe) exec '{}' \"$@\";; esac\n{body}\n",
            real.display(),
            real.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn closed_image(tool: &Path) {
    let f = closed();
    let image = link_with(&f, tool, 32 * 1024 * 1024).unwrap();
    let manifest = &image.manifest;
    assert_eq!(manifest.entry, 0x10000000);
    assert_eq!(manifest.request, f.request);
    assert_eq!(manifest.roots[0].symbol, f.request.entry);
    assert_eq!(&manifest.elf, image.elf.id());
    assert_eq!(&image.elf.bytes()[..4], b"\x7fELF");
    assert!(
        image
            .elf
            .bytes()
            .windows(4)
            .any(|b| b == [0x78, 0x56, 0x34, 0x12])
    );
    assert!(!image.map.is_empty());
    // The lazily extracted helper member is placed and identified by content.
    let helper = ObjectId {
        artifact: f.inputs[1].id().clone(),
        location: ObjectLocation::ArchiveMember { ordinal: 0 },
    };
    assert!(
        manifest
            .mappings
            .iter()
            .any(|m| m.object == helper && m.payload == ArtifactId::of_bytes(&object(false)))
    );
    // Linking is deterministic: the same request links the same image.
    let again = link_with(&f, tool, 32 * 1024 * 1024).unwrap();
    assert_eq!(again.manifest, image.manifest);
    // The temporary link directory is removed.
    assert_eq!(fs::read_dir(f.dir.path()).unwrap().count(), 0);
}

#[test]
fn real_lld_links_a_closed_image() {
    closed_image(&linker());
}

#[test]
fn real_gnu_links_a_closed_image() {
    closed_image(&gnu_linker());
}

#[test]
fn unresolved_names_and_conflicting_definitions_fail_the_link() {
    let f = fixture(
        vec![support::archive(&[(b"entry.o", &object(true))], false)],
        0,
        0,
    );
    assert_eq!(linked(&f).err().unwrap().code, ErrorCode::LinkFailed);
    let f = fixture(
        vec![
            object(true),
            object(false),
            dependency_object(b"helper", None),
        ],
        0,
        0,
    );
    assert_eq!(linked(&f).err().unwrap().code, ErrorCode::LinkFailed);
}

#[test]
fn repeated_member_bytes_and_additional_roots_keep_exact_occurrences() {
    let entry = object(true);
    let f = fixture(
        vec![
            support::archive(&[(b"same.o", &entry), (b"same.o", &entry)], false),
            object(false),
        ],
        0,
        1,
    );
    let image = linked(&f).unwrap();
    assert_eq!(
        image.manifest.roots[0].symbol.object.location,
        ObjectLocation::ArchiveMember { ordinal: 1 }
    );
    let mut rooted = fixture(vec![object(true), object(false)], 0, 0);
    rooted
        .request
        .roots
        .push(defined(&rooted.inputs[1], 0, b"helper"));
    let image = linked(&rooted).unwrap();
    assert_eq!(image.manifest.roots.len(), 2);
    assert_eq!(image.manifest.roots[1].name, b"helper");
}

#[test]
fn inputs_are_distinct_given_executables_holding_every_root() {
    let mut f = closed();
    f.request.inputs.push(f.request.inputs[0].clone());
    assert_eq!(linked(&f).err().unwrap().code, ErrorCode::InvalidRequest);
    let mut f = closed();
    let absent = f.inputs.pop().unwrap();
    assert!(!f.inputs.iter().any(|e| e.id() == absent.id()));
    let error = linked(&f).err().unwrap();
    assert_eq!(error.code, ErrorCode::LinkBlocked);
    assert!(error.message.contains("not given"), "{error:?}");
    // An executable no request names is never read.
    let mut f = closed();
    f.inputs
        .push(Executable::new(b"corrupt and unrelated".to_vec()));
    linked(&f).unwrap();
}

#[test]
fn a_companion_defines_a_name_no_link_input_may_define() {
    let helper = Executable::new(support::executable_with_symbols(
        &[0x00008067],
        &[("rom_helper", 0x1000, 4)],
    ));
    let clash = Executable::new(support::executable_with_symbols(
        &[0x00008067],
        &[("entry", 0x1000, 4)],
    ));
    let mut f = fixture(vec![dependency_object(b"entry", None)], 0, 0);
    f.inputs.extend([helper.clone(), clash.clone()]);
    f.request.companions = vec![defined(&helper, 0, b"rom_helper")];
    linked(&f).unwrap();
    f.request.companions = vec![defined(&clash, 0, b"entry")];
    assert_eq!(linked(&f).err().unwrap().code, ErrorCode::Conflict);
}

#[test]
fn unsupported_abi_map_line_injection_and_thin_members_block_the_link() {
    for mutation in [0, 1] {
        let mut helper = object(false);
        if mutation == 0 {
            helper[36..40].copy_from_slice(&object::elf::EF_RISCV_RVE.to_le_bytes());
        } else {
            let offset = helper.windows(6).position(|s| s == b"helper").unwrap();
            helper[offset] = b'\n';
        }
        let f = fixture(vec![object(true), helper], 0, 0);
        assert_eq!(linked(&f).err().unwrap().code, ErrorCode::LinkBlocked);
    }
    let f = fixture(
        vec![
            object(true),
            support::archive(&[(b"absent.o", &object(false))], true),
        ],
        0,
        0,
    );
    assert_eq!(linked(&f).err().unwrap().code, ErrorCode::LinkBlocked);
}

fn helper_with_common(weak: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.helper".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[0x67, 0x80, 0, 0], 4);
    let mut helper = symbol(b"helper", SymbolSection::Section(text), 4, SymbolKind::Text);
    helper.weak = weak;
    obj.add_symbol(helper);
    let mut data = symbol(b"value", SymbolSection::Common, 4, SymbolKind::Data);
    data.value = 4;
    obj.add_symbol(data);
    obj.write().unwrap()
}

#[test]
fn weak_definitions_and_common_data_link_under_declared_policy() {
    let f = fixture(
        vec![
            object(true),
            support::archive(&[(b"helper.o", &helper_with_common(true))], false),
        ],
        0,
        0,
    );
    let image = linked(&f).unwrap();
    assert!(
        image
            .manifest
            .segments
            .iter()
            .any(|s| s.memory_size > s.file_size)
    );
}

#[test]
fn working_capacity_and_bad_linker_output_fail_closed() {
    let f = closed();
    let error = link_with(&f, &linker(), 1024 * 1024).err().unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert!(error.memory.is_some());
    let tool = test_linker(f.dir.path(), "printf 'not an ELF'");
    assert_eq!(
        link_with(&f, &tool, 32 * 1024 * 1024).err().unwrap().code,
        ErrorCode::LinkBlocked
    );
}

#[test]
fn linker_stderr_flood_is_bounded_in_the_failure() {
    let f = closed();
    let tool = test_linker(
        f.dir.path(),
        "/usr/bin/head -c 131072 /dev/zero >&2; exit 42",
    );
    let error = link_with(&f, &tool, 32 * 1024 * 1024).err().unwrap();
    assert_eq!(error.code, ErrorCode::LinkFailed);
    assert!(error.message.contains("42"), "{}", error.message);
    assert!(error.message.len() < 2048);
}

#[test]
fn gnu_output_limit_crash_and_deadline_fail_and_reap_the_linker() {
    for (body, expected) in [
        (
            "exec /usr/bin/head -c 67108864 /dev/zero > image.elf",
            ErrorCode::ResourceLimited,
        ),
        ("kill -SEGV $$", ErrorCode::LinkFailed),
        ("exec /bin/sleep 30", ErrorCode::ResourceLimited),
    ] {
        let f = closed();
        let tool = gnu_test_linker(f.dir.path(), body);
        let timeout = if body.contains("sleep") { 1 } else { 30 };
        let started = std::time::Instant::now();
        let error = link(
            &f.request,
            &f.inputs,
            &tool,
            &ElfLinker,
            f.dir.path(),
            &WorkingMemory::new(32 * 1024 * 1024).unwrap(),
            &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(timeout)),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, expected, "{body}: {error:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }
}

#[test]
fn archive_order_decides_gnu_extraction() {
    let f = fixture(
        vec![
            support::archive(&[(b"target.o", &dependency_object(b"target", None))], false),
            support::archive(
                &[(b"bridge.o", &dependency_object(b"bridge", Some(b"target")))],
                false,
            ),
            dependency_object(b"entry", Some(b"bridge")),
        ],
        2,
        0,
    );
    let lld = linked(&f).unwrap();
    assert_eq!(lld.manifest.linker.implementation, "lld-elf");
    let gnu = link_with(&f, &gnu_linker(), 32 * 1024 * 1024);
    assert_eq!(gnu.err().unwrap().code, ErrorCode::LinkFailed);
}

#[test]
fn capabilities_not_version_numbers_decide_adapter_support() {
    use std::os::unix::fs::PermissionsExt;
    for (version, real) in [
        ("LLD 99.7", linker()),
        ("GNU ld (future) 99.7", gnu_linker().canonicalize().unwrap()),
    ] {
        let f = closed();
        let path = f.dir.path().join("future-linker");
        fs::write(&path, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\nexec '{}' \"$@\"\n", real.display())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let image = link_with(&f, &path, 32 * 1024 * 1024).unwrap();
        assert_eq!(image.manifest.linker.version, version);
        fs::write(
            &path,
            format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; fi\nexit 0\n"),
        )
        .unwrap();
        let error = link_with(&f, &path, 32 * 1024 * 1024).err().unwrap();
        assert_eq!(error.code, ErrorCode::Incompatible);
    }
}

#[test]
fn gnu_code_only_image_retains_empty_segment_without_mapping_memory() {
    let f = fixture(vec![dependency_object(b"entry", None)], 0, 0);
    let image = link_with(&f, &gnu_linker(), 32 * 1024 * 1024).unwrap();
    assert!(
        image
            .manifest
            .segments
            .iter()
            .any(|s| s.memory_size == 0 && s.file_size == 0)
    );
}

#[test]
fn oversized_map_record_and_nondeterministic_capability_evidence_fail() {
    use std::os::unix::fs::PermissionsExt;
    let f = closed();
    let tool = test_linker(
        f.dir.path(),
        "for arg in \"$@\"; do case \"$arg\" in --Map=*) map=${arg#--Map=};; esac; done\nif [ -n \"$map\" ]; then exec /usr/bin/head -c 70000 /dev/zero > \"$map\"; else exec /usr/bin/head -c 70000 /dev/zero; fi",
    );
    assert_eq!(
        link_with(&f, &tool, 32 * 1024 * 1024).err().unwrap().code,
        ErrorCode::ResourceLimited
    );
    let real = linker();
    let varying = f.dir.path().join("varying-linker");
    fs::write(&varying, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then exec '{}' --version; fi\necho \"$PWD\" >&2\nexec '{}' \"$@\"\n", real.display(), real.display())).unwrap();
    fs::set_permissions(&varying, fs::Permissions::from_mode(0o700)).unwrap();
    let error = link_with(&f, &varying, 32 * 1024 * 1024).err().unwrap();
    assert_eq!(error.code, ErrorCode::Incompatible);
    assert!(error.message.contains("deterministic"));
}

/// `entry` calling `helper` and storing the addresses of `value` and, when
/// `missing` is set, of an undefined `missing`.
fn proposal_object(missing: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(
        section,
        &[
            0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x67, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
        4,
    );
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(section),
        20,
        SymbolKind::Text,
    ));
    let mut references = vec![(0, b"helper".as_slice(), object::elf::R_RISCV_CALL)];
    references.push((12, b"value", object::elf::R_RISCV_32));
    if missing {
        references.push((16, b"missing", object::elf::R_RISCV_32));
    }
    for (offset, name, r_type) in references {
        // Default visibility, as in vendor objects; a hidden undefined name
        // cannot be left for a companion.
        let target = obj.add_symbol(Symbol {
            scope: SymbolScope::Dynamic,
            ..symbol(name, SymbolSection::Undefined, 0, SymbolKind::Unknown)
        });
        obj.add_relocation(
            section,
            Relocation {
                offset,
                symbol: target,
                addend: 0,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}

#[test]
fn trial_link_proposes_function_and_data_companions_and_reports_the_rest() {
    // A linked image defining `helper` and `value` serves as the ROM.
    let rom = linked(&closed()).unwrap().elf;
    let propose = |f: &Fixture, candidates: &[ArtifactId]| {
        propose_companions(
            &f.request,
            candidates,
            &f.inputs,
            &linker(),
            &ElfLinker,
            f.dir.path(),
            &WorkingMemory::new(32 * 1024 * 1024).unwrap(),
            &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30)),
        )
    };
    let mut with_missing = fixture(vec![proposal_object(true)], 0, 0);
    with_missing.inputs.push(rom.clone());
    with_missing.request.layout.code.start = 0x30000000;
    with_missing.request.layout.data.start = 0x31000000;
    let proposal = propose(&with_missing, std::slice::from_ref(rom.id())).unwrap();
    assert_eq!(proposal.unresolved, ["missing"]);
    let names: Vec<_> = proposal.resolved.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["helper", "value"]);
    assert!(
        proposal
            .resolved
            .iter()
            .all(|c| c.symbol.object.artifact == *rom.id())
    );
    // Without the undefined name, the proposal alone closes the image.
    let mut f = fixture(vec![proposal_object(false)], 0, 0);
    f.inputs.push(rom.clone());
    f.request.layout = with_missing.request.layout;
    let proposal = propose(&f, std::slice::from_ref(rom.id())).unwrap();
    assert!(proposal.unresolved.is_empty());
    f.request.companions = proposal.resolved.into_iter().map(|c| c.symbol).collect();
    assert_eq!(linked(&f).unwrap().manifest.request.companions.len(), 2);
    // A candidate that is also a link input is rejected before linking.
    let input = f.request.inputs[0].clone();
    assert_eq!(
        propose(&f, &[input]).err().unwrap().code,
        ErrorCode::InvalidRequest
    );
}
