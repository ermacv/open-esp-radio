//! In-process inventory of captured executables, identified by content.
#[allow(dead_code)]
mod support;
use blobray_application::captured::inventory;
use blobray_application::in_process::{Executable, Limits};
use blobray_domain::*;
use oer_riscv_model::*;

fn inventory_of(bytes: Vec<u8>) -> (Executable, ArtifactInventory) {
    let executable = Executable::new(bytes);
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut control = Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30));
    let inventory = inventory(&executable, &memory, &mut control).unwrap();
    (executable, inventory)
}

#[test]
fn archive_members_are_occurrences_of_the_archive_content() {
    let elf = support::elf();
    let (archive, inventory) = inventory_of(support::archive(
        &[
            (b"same.o", &elf),
            (b"same.o", &elf),
            (b"notes.txt", b"text"),
        ],
        false,
    ));
    assert_eq!(inventory.kind, ContainerKind::Archive);
    assert!(inventory.members_complete);
    assert_eq!(inventory.objects.len(), 3);
    for (ordinal, object) in inventory.objects.iter().enumerate() {
        assert_eq!(object.id.artifact, *archive.id());
        assert_eq!(
            object.id.location,
            ObjectLocation::ArchiveMember {
                ordinal: ordinal as u64
            }
        );
    }
    // Repeated names and bytes stay distinct occurrences of one content.
    assert_eq!(inventory.objects[0].name.as_deref(), Some(&b"same.o"[..]));
    assert_eq!(
        inventory.objects[0].content,
        Some(ArtifactId::of_bytes(&elf))
    );
    assert_eq!(inventory.objects[0].content, inventory.objects[1].content);
    let symbols = &inventory.objects[1].elf.as_ref().unwrap().symbols;
    assert!(
        symbols
            .iter()
            .all(|s| s.id.object == inventory.objects[1].id)
    );
    assert!(
        symbols
            .iter()
            .any(|s| s.name.as_deref() == Some(&b"same"[..]))
    );
    // A member that is no ELF keeps its bytes' identity and a diagnostic.
    let notes = &inventory.objects[2];
    assert!(notes.elf.is_none());
    assert_eq!(notes.content, Some(ArtifactId::of_bytes(b"text")));
    assert!(
        notes
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::UnsupportedFormat)
    );
    assert!(!inventory.complete());
}

#[test]
fn a_standalone_elf_is_one_object_with_its_tables() {
    let elf = support::elf();
    let (executable, inventory) = inventory_of(elf.clone());
    assert_eq!(inventory.kind, ContainerKind::Elf);
    assert_eq!(inventory.objects.len(), 1);
    let object = &inventory.objects[0];
    assert_eq!(object.id.location, ObjectLocation::Standalone);
    assert_eq!(object.content.as_ref(), Some(executable.id()));
    let tables = object.elf.as_ref().unwrap();
    assert!(!tables.sections.is_empty() && !tables.symbols.is_empty());
    assert!(!tables.relocations.is_empty());
    assert!(inventory.complete());
}

#[test]
fn thin_members_and_broken_framing_stay_visible_as_incomplete() {
    let elf = support::elf();
    let (_, thin) = inventory_of(support::archive(&[(b"member.o", &elf)], true));
    assert_eq!(thin.kind, ContainerKind::ThinArchive);
    let member = &thin.objects[0];
    assert!(member.content.is_none() && member.elf.is_none());
    assert_eq!(member.diagnostics[0].code, DiagnosticCode::MissingMember);
    assert!(!thin.complete());

    let mut truncated = support::archive(&[(b"first.o", &elf), (b"second.o", &elf)], false);
    truncated.truncate(truncated.len() - 3);
    // A member whose bytes end early is still a known member, without ELF.
    let (_, short) = inventory_of(truncated);
    assert!(short.members_complete);
    assert_eq!(short.objects.len(), 2);
    assert!(short.objects[0].elf.is_some());
    assert!(short.objects[1].elf.is_none());
    assert!(!short.objects[1].diagnostics.is_empty(), "{short:?}");
    assert!(!short.complete());

    // Broken member framing leaves the remaining membership unknown.
    let (_, header) = inventory_of(b"!<arch>\nnot a member header".to_vec());
    assert!(!header.members_complete);
    assert!(header.objects.is_empty());
    assert_eq!(
        header.diagnostics[0].code,
        DiagnosticCode::MalformedContainer
    );
    assert!(!header.complete());
}

#[test]
fn exhausted_budgets_fail_the_inventory() {
    let executable = Executable::new(support::archive(&[(b"a.o", &support::elf())], false));
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let mut control = Limits::new(1, std::time::Duration::from_secs(30));
    let error = inventory(&executable, &memory, &mut control).unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    let small = WorkingMemory::new(1024).unwrap();
    let mut control = Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30));
    let error = inventory(&executable, &small, &mut control).unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
}
