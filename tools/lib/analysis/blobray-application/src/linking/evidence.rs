//! Validate adapter claims against captured occurrences; no tool text is interpreted.
use super::*;

/// The roots and placed sections the linker's claims establish, checked in
/// the canonical order extraction, placement, exit, independent of pipe
/// polling.
pub(super) fn roots(
    outputs: &Outputs,
    found: &Collected,
    invocation: &LinkInvocation<'_>,
    control: &mut dyn RunControl,
) -> Result<(Vec<ResolvedRoot>, Vec<ImageMapping>)> {
    if outputs.exit_observations != 1 {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            "missing linker exit evidence",
        ));
    }
    let exit = LinkObservation::ToolExit {
        code: outputs.exit_code,
        signal: outputs.signal,
    };
    let mut addresses = vec![None; found.roots.len()];
    let mut placed = vec![false; found.members.len()];
    let mut extracted = vec![false; found.members.len()];
    let mut mappings = Vec::new();
    let mut exited = false;
    let member = |object: &ObjectId| {
        found
            .members
            .iter()
            .position(|m| &m.object == object)
            .ok_or_else(|| invalid("linker observation names an unknown occurrence"))
    };
    let span = |evidence: &LinkEvidenceSpan| -> Result<()> {
        let length = match evidence.source {
            LinkEvidenceSource::Map => outputs.map.len(),
            LinkEvidenceSource::Extraction => outputs.extraction.len(),
        } as u64;
        if evidence.length == 0
            || evidence
                .offset
                .checked_add(evidence.length)
                .is_none_or(|end| end > length)
        {
            return Err(invalid("link evidence lies outside retained raw output"));
        }
        Ok(())
    };
    for observation in outputs
        .extractions
        .iter()
        .chain(&outputs.placements)
        .chain(std::iter::once(&exit))
    {
        control.checkpoint(1)?;
        match observation {
            LinkObservation::SectionPlacement {
                object,
                section,
                address,
                size,
                evidence,
            } => {
                span(evidence)?;
                if address
                    .checked_add(*size)
                    .is_none_or(|end| end > 1u64 << 32)
                {
                    return Err(invalid("link placement exceeds RV32"));
                }
                let index = member(object)?;
                placed[index] |= *size != 0;
                let m = &found.members[index];
                let mut exact = false;
                for (index, root) in found.roots.iter().enumerate() {
                    if root.symbol.object == m.object && &root.section == section {
                        if root.section_size != *size || addresses[index].is_some() {
                            return Err(Error::new(
                                ErrorCode::LinkBlocked,
                                "root section placement is transformed or ambiguous",
                            ));
                        }
                        addresses[index] = Some(
                            address
                                .checked_add(root.offset)
                                .ok_or_else(|| invalid("root address overflow"))?,
                        );
                        exact = true;
                    }
                }
                mappings.push(ImageMapping {
                    object: m.object.clone(),
                    payload: m
                        .payload
                        .clone()
                        .ok_or_else(|| invalid("placed member without captured bytes"))?,
                    section: section.clone(),
                    address: *address,
                    size: *size,
                    exact,
                });
            }
            LinkObservation::ArchiveExtraction {
                object,
                cause,
                referring,
                evidence,
            } => {
                span(evidence)?;
                let index = member(object)?;
                let alias = &found.members[index].alias;
                if !invocation
                    .inputs
                    .iter()
                    .any(|i| matches!(i, LinkInput::Archive(paths) if paths.contains(alias)))
                    || extracted[index]
                {
                    return Err(invalid("invalid or duplicate archive extraction"));
                }
                if cause
                    .as_ref()
                    .is_none_or(|s| s.is_empty() || s.len() > 4096)
                {
                    return Err(invalid("missing archive extraction cause"));
                }
                if let Some(referring) = referring {
                    member(referring)?;
                }
                extracted[index] = true;
            }
            LinkObservation::ToolExit { code, signal } => {
                if exited
                    || *code != Some(0)
                    || signal.is_some()
                    || *code != outputs.exit_code
                    || *signal != outputs.signal
                {
                    return Err(invalid("invalid linker exit observation"));
                }
                exited = true;
            }
        }
    }
    for (index, m) in found.members.iter().enumerate() {
        let lazy = invocation
            .inputs
            .iter()
            .any(|i| matches!(i, LinkInput::Archive(paths) if paths.contains(&m.alias)));
        if placed[index] && lazy && !extracted[index] {
            return Err(Error::new(
                ErrorCode::LinkBlocked,
                "missing archive extraction evidence",
            ));
        }
    }
    let roots = found
        .roots
        .iter()
        .zip(addresses)
        .map(|(root, address)| {
            Ok(ResolvedRoot {
                symbol: root.symbol.clone(),
                name: root.name.clone(),
                size: root.size,
                address: address.ok_or_else(|| {
                    Error::new(
                        ErrorCode::LinkBlocked,
                        "no exact linker provenance for a requested root",
                    )
                })?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((roots, mappings))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn validate(mut change: impl FnMut(&mut Vec<LinkObservation>)) -> Result<Vec<ResolvedRoot>> {
        let dir = tempfile::tempdir().unwrap();
        let workspace = LinkWorkspace::within(dir.path(), 65536).unwrap();
        let id = ArtifactId::of_bytes(b"captured source");
        let object = ObjectId {
            artifact: id.clone(),
            location: ObjectLocation::Standalone,
        };
        let selected = SymbolId {
            object: object.clone(),
            table: SymbolTableKind::Static,
            table_section: 1,
            index: 1,
        };
        let found = Collected {
            blockers: vec![],
            definitions: vec![],
            members: vec![Member {
                input: 0,
                object: object.clone(),
                payload: Some(id.clone()),
                alias: "root.o".into(),
            }],
            roots: vec![blobray_artifacts::LinkRootFacts {
                symbol: selected,
                name: b"root".to_vec(),
                section: b"code".to_vec(),
                section_size: 16,
                offset: 4,
                size: 8,
            }],
        };
        let identity = LinkerIdentity {
            implementation: "test-capability".into(),
            version: "future".into(),
            executable: id,
        };
        let invocation = LinkInvocation {
            unresolved: UnresolvedSymbols::Error,
            executable: Path::new("unused"),
            identity: &identity,
            contract: LinkerContract::ElfAnalysisLinkV1,
            workspace: &workspace,
            entry: b"root",
            roots: vec![],
            forced: vec!["root.o".into()],
            inputs: vec![],
            members: vec![LinkMember {
                alias: "root.o".into(),
                object: object.clone(),
            }],
            layout: ImageLayout {
                code: ImageRegion {
                    start: 0x1000,
                    length: 4096,
                },
                data: ImageRegion {
                    start: 0x2000,
                    length: 4096,
                },
            },
            definitions: vec![],
        };
        let mut outputs = Outputs::default();
        outputs
            .write(
                LinkOutput::Map,
                b"opaque tool dialect; application cannot parse this",
                &mut || Ok(()),
            )
            .unwrap();
        let mut records = vec![
            LinkObservation::SectionPlacement {
                object,
                section: b"code".to_vec(),
                address: 0x1000,
                size: 16,
                evidence: LinkEvidenceSpan {
                    source: LinkEvidenceSource::Map,
                    offset: 0,
                    length: 6,
                },
            },
            LinkObservation::ToolExit {
                code: Some(0),
                signal: None,
            },
        ];
        change(&mut records);
        outputs.exited(Some(0), None);
        for record in records {
            outputs.observe(record, &mut || Ok(()))?;
        }
        roots(&outputs, &found, &invocation, &mut || Ok(())).map(|(r, _)| r)
    }
    #[test]
    fn roots_use_typed_claims_and_captured_offsets_not_a_tool_dialect() {
        let roots = validate(|_| {}).unwrap();
        assert_eq!(roots[0].address, 0x1004);
        assert_eq!(roots[0].symbol.index, 1);
    }
    #[test]
    fn missing_ambiguous_transformed_foreign_or_unbacked_claims_fail_closed() {
        assert!(
            validate(|r| {
                r.remove(0);
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                r.insert(0, r[0].clone());
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { size, .. } = &mut r[0] {
                    *size = 12;
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { object, .. } = &mut r[0] {
                    object.location = ObjectLocation::ArchiveMember { ordinal: 1 };
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { evidence, .. } = &mut r[0] {
                    evidence.length = 1000;
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                r.pop();
            })
            .is_err()
        );
    }
    #[test]
    fn forced_member_cannot_impersonate_archive_extraction() {
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement {
                    object, evidence, ..
                } = r[0].clone()
                {
                    r.push(LinkObservation::ArchiveExtraction {
                        object,
                        evidence,
                        cause: Some(b"root".to_vec()),
                        referring: None,
                    });
                }
            })
            .is_err()
        );
    }
}
