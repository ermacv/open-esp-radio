use super::*;
use blobray_application::linking::LinkMember;
use records::Parser;
#[derive(Default)]
struct Sink(Vec<LinkObservation>);
impl LinkOutputSink for Sink {
    fn write(&mut self, _: LinkOutput, _: &[u8], _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn observe(&mut self, r: LinkObservation, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(r);
        Ok(())
    }
    fn elf(&mut self, _: Vec<u8>) -> Result<()> {
        Ok(())
    }
}
fn object(alias: &str) -> ObjectId {
    ObjectId {
        artifact: ArtifactId::of_bytes(alias.as_bytes()),
        location: ObjectLocation::Standalone,
    }
}
fn members() -> Vec<LinkMember> {
    ["root.o", "helper.o"]
        .into_iter()
        .map(|alias| LinkMember {
            alias: alias.into(),
            object: object(alias),
        })
        .collect()
}
fn lines(
    parser: &mut impl Parser,
    channel: LinkOutput,
    text: &[u8],
    sink: &mut Sink,
) -> Result<()> {
    let mut offset = 0;
    for line in text.split_inclusive(|b| *b == b'\n') {
        parser.line(
            channel,
            line,
            LinkEvidenceSpan {
                source: if channel == LinkOutput::Map {
                    LinkEvidenceSource::Map
                } else {
                    LinkEvidenceSource::Extraction
                },
                offset,
                length: line.len() as u64,
            },
            &members(),
            sink,
            &mut || Ok(()),
        )?;
        offset += line.len() as u64;
    }
    Ok(())
}
#[test]
fn lld_symbols_cannot_impersonate_input_sections_and_extraction_is_typed() {
    let mut sink = Sink::default();
    let mut parser = lld::Parser;
    lines(&mut parser, LinkOutput::Map, b"1000 1000 10 4         root.o:(.text root)\n1000 1000 10 4                 helper.o:(.text.fake)\n", &mut sink).unwrap();
    assert_eq!(sink.0.len(), 1);
    assert!(
        matches!(&sink.0[0], LinkObservation::SectionPlacement { section, address: 0x1000, .. } if section == b".text root")
    );
    lines(
        &mut parser,
        LinkOutput::Extraction,
        b"reference\textracted\tsymbol\nroot.o\thelper.o\thelper\n",
        &mut sink,
    )
    .unwrap();
    assert!(matches!(
        &sink.0[1],
        LinkObservation::ArchiveExtraction {
            referring: Some(referring),
            object: extracted,
            ..
        } if referring == &object("root.o") && extracted == &object("helper.o")
    ));
    assert!(lines(&mut parser, LinkOutput::Extraction, b"broken\n", &mut sink).is_err());
    assert!(
        lines(
            &mut parser,
            LinkOutput::Map,
            b"1000 1000 10 4         unknown.o:(.text)\n",
            &mut sink
        )
        .is_err()
    );
}
#[test]
fn gnu_discards_output_symbols_and_wildcards_are_not_placements() {
    let mut sink = Sink::default();
    let mut parser = gnu_ld::Parser::default();
    lines(&mut parser, LinkOutput::Map, b"Archive member included to satisfy reference by file (symbol)\n\nhelper.o\n                              root.o (helper)\n\nDiscarded input sections\n .text.discard  0x0 0x4 helper.o\nLinker script and memory map\n.text 0x1000 0x14\n *(*)\n .text.root     0x1000 0x10 root.o\n                0x1000                fake.o:(.text)\n .text.long section name\n                0x1010 0x4 helper.o\n *fill* 0x1014 0xc\n", &mut sink).unwrap();
    assert_eq!(sink.0.len(), 3);
    assert!(
        matches!(&sink.0[0], LinkObservation::ArchiveExtraction { cause: Some(cause), .. } if cause == b"helper")
    );
    assert!(
        matches!(&sink.0[2], LinkObservation::SectionPlacement { section, address: 0x1010, evidence, .. } if section == b".text.long section name" && evidence.length > 40)
    );
    assert!(
        lines(
            &mut parser,
            LinkOutput::Map,
            b" .bad 0xNOTHEX 0x4 root.o\n",
            &mut sink
        )
        .is_err()
    );
}

#[test]
fn version_diagnostics_are_bounded_and_do_not_change_family_detection() {
    let mut version = Version {
        stdout: Vec::new(),
        total: 0,
    };
    version
        .write(LinkOutput::Stderr, b"warning\n", &mut || Ok(()))
        .unwrap();
    version
        .write(LinkOutput::Elf, b"LLD 99.0\n", &mut || Ok(()))
        .unwrap();
    assert_eq!(version.stdout, b"LLD 99.0\n");
    assert_eq!(
        version
            .write(LinkOutput::Stderr, &[b'x'; 4096], &mut || Ok(()))
            .unwrap_err()
            .code,
        ErrorCode::Incompatible
    );
}
#[test]
fn truncated_gnu_continuations_are_rejected() {
    let mut parser = gnu_ld::Parser::default();
    lines(
        &mut parser,
        LinkOutput::Map,
        b"Linker script and memory map\n .text.unfinished\n",
        &mut Sink::default(),
    )
    .unwrap();
    assert!(parser.finish().is_err());
}

#[test]
fn unknown_gnu_extraction_cannot_be_silently_treated_as_a_map_heading() {
    let mut parser = gnu_ld::Parser::default();
    let error = lines(&mut parser, LinkOutput::Map, b"Archive member included to satisfy reference by file (symbol)\n\nforeign.o                     root.o (helper)\n", &mut Sink::default()).unwrap_err();
    assert_eq!(error.code, ErrorCode::LinkBlocked);
    assert!(error.message.contains("extraction"));
}
