//! Captured executables read in process, identified by content: their
//! container members, object inventory and bytes.
use crate::in_process::Executable;
use crate::*;

/// One object of a captured executable, borrowed for a visit.
pub(crate) struct Member<'a> {
    pub id: ObjectId,
    pub name: Option<&'a [u8]>,
    /// The object's bytes, or why the container holds none: a thin archive
    /// member names an external file, and a member range may be malformed.
    pub bytes: std::result::Result<&'a dyn ByteSource, Diagnostic>,
}

/// Container framing of a captured executable.
pub(crate) struct Container {
    pub kind: ContainerKind,
    /// Framing that stops member enumeration; later members are unknown.
    pub framing: Option<Diagnostic>,
}

type MemberVisit<'v> = dyn FnMut(Member<'_>, &mut dyn RunControl) -> Result<()> + 'v;

/// Present every object of `executable` to `visit` in container order.
pub(crate) fn visit_members(
    executable: &Executable,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut MemberVisit<'_>,
) -> Result<Container> {
    let source: &[u8] = executable.bytes();
    let mut cursor = match MemberCursor::new(&source, control) {
        Ok(cursor) => cursor,
        Err(error) if error.code == ErrorCode::Integrity => {
            return Ok(Container {
                kind: if source.starts_with(b"!<thin>\n") {
                    ContainerKind::ThinArchive
                } else {
                    ContainerKind::Archive
                },
                framing: Some(Diagnostic {
                    code: DiagnosticCode::MalformedContainer,
                    context: "archive header; membership unknown".into(),
                    message: error.message,
                }),
            });
        }
        Err(error) => return Err(error),
    };
    let kind = cursor.kind;
    loop {
        let member = match cursor.next(memory, control) {
            Ok(Some(member)) => member,
            Ok(None) => break,
            Err(error) if error.code == ErrorCode::Integrity => {
                return Ok(Container {
                    kind,
                    framing: Some(Diagnostic {
                        code: DiagnosticCode::MalformedContainer,
                        context: "archive member; remaining membership unknown".into(),
                        message: error.message,
                    }),
                });
            }
            Err(error) => return Err(error),
        };
        let mut position = control.position();
        position.member = Some(member.ordinal);
        position.table = None;
        position.entry = None;
        control.set_position(position);
        let name = member.name.as_deref();
        let id = ObjectId {
            artifact: executable.id().clone(),
            location: match kind {
                ContainerKind::Archive | ContainerKind::ThinArchive => {
                    ObjectLocation::ArchiveMember {
                        ordinal: member.ordinal,
                    }
                }
                _ => ObjectLocation::Standalone,
            },
        };
        match member.payload {
            Some((offset, length)) => match SourceRange::new(&source, offset, length) {
                Ok(range) => visit(
                    Member {
                        id,
                        name,
                        bytes: Ok(&range),
                    },
                    control,
                )?,
                Err(error) => visit(
                    Member {
                        id,
                        name,
                        bytes: Err(Diagnostic {
                            code: DiagnosticCode::MalformedObject,
                            context: "member payload".into(),
                            message: error.message,
                        }),
                    },
                    control,
                )?,
            },
            None => visit(
                Member {
                    id,
                    name,
                    bytes: Err(Diagnostic {
                        code: DiagnosticCode::MissingMember,
                        context: "thin member".into(),
                        message: "a thin archive names its member files; give the members \
                                  themselves"
                            .into(),
                    }),
                },
                control,
            )?,
        }
    }
    Ok(Container {
        kind,
        framing: None,
    })
}

/// Every ELF record of one object, materialized.
#[derive(Default)]
struct Tables {
    sections: Vec<SectionRecord>,
    symbols: Vec<SymbolRecord>,
    relocations: Vec<RelocationRecord>,
    diagnostics: Vec<Diagnostic>,
}
impl ElfSink for Tables {
    fn section(&mut self, record: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.sections.push(record.clone());
        Ok(())
    }
    fn symbol(&mut self, record: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        self.symbols.push(record.clone());
        Ok(())
    }
    fn relocation(&mut self, record: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        self.relocations.push(record.clone());
        Ok(())
    }
    fn diagnostic(&mut self, record: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(record.clone());
        Ok(())
    }
}

/// The inventory of one object: its content identity, ELF tables and
/// diagnostics.
pub(crate) fn object_inventory(
    member: Member<'_>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ObjectInventory> {
    let mut tables = Tables::default();
    let (content, elf) = match member.bytes {
        Ok(bytes) => {
            let (content, elf) = inspect_source(bytes, &member.id, memory, control, &mut tables)?;
            (Some(content), elf)
        }
        Err(diagnostic) => {
            tables.diagnostics.push(diagnostic);
            (None, None)
        }
    };
    Ok(ObjectInventory {
        id: member.id,
        name: member.name.map(<[u8]>::to_vec),
        content,
        elf: elf.map(|elf| ElfInventory {
            sections: tables.sections,
            symbols: tables.symbols,
            relocations: tables.relocations,
            ..elf
        }),
        diagnostics: tables.diagnostics,
    })
}

/// The inventory of every object `executable` contains, in container order.
pub fn inventory(
    executable: &Executable,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ArtifactInventory> {
    let mut objects = Vec::new();
    let container = visit_members(executable, memory, control, &mut |member, c| {
        objects.push(object_inventory(member, memory, c)?);
        Ok(())
    })?;
    Ok(ArtifactInventory {
        kind: container.kind,
        members_complete: container.framing.is_none(),
        objects,
        diagnostics: container.framing.into_iter().collect(),
    })
}
