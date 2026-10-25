//! Pointer, complete value-domain and status/clear facts are checked without policy.

use super::*;

const FACTS: &str = r#"
[[registers]]
name = "DMA"
offset = 0xc0

[[registers.fields]]
name = "ADDRESS"
bits = "19:0"
class = "plain"
pointer = { points-to = "RX_DESCRIPTOR", placement = "dma-window", member-offset = 4, encoding = { width = 20, shift = 2 }, sources = ["SRC_USE"] }

[[registers]]
name = "HIGH"
offset = 0xc4

[[registers.fields]]
name = "PREFIX"
bits = "31:20"
class = "plain"

[[registers]]
name = "DOMAIN"
offset = 0xc8

[[registers.fields]]
name = "VALUE"
bits = "3:0"
class = "read-only"
value-range = { min = 0, max = 15, sources = ["SRC_HEADER"] }

[[registers.fields]]
name = "ENUM"
bits = "5:4"
class = "plain"
values = [{ name = "IDLE", value = 0, sources = ["SRC_HEADER"] }, { name = "RUN", value = 3, sources = ["SRC_USE"] }]
write-values-only = true

[[registers]]
name = "ACK"
offset = 0xcc

[[registers.fields]]
name = "T0"
bits = "0"
class = "write-one-to-clear"

[[registers.fields]]
name = "T1"
bits = "1"
class = "write-one-to-clear"

[[interrupt-pairs]]
status = "INT_RAW"
clear = "ACK"
sources = ["SRC_HEADER"]
"#;

fn fixture() -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture.write("types/timg.toml", &format!("{TIMG}\n{FACTS}"))?;
    Ok(fixture)
}

#[test]
fn facts_resolve_on_repeated_instances_without_policy() -> TestResult {
    let model = fixture()?.load()?;
    for instance in ["TIMG0", "TIMG1"] {
        assert!(model.field(&format!("{instance}.DMA.ADDRESS")).is_some());
    }
    let field = &model.types()["TIMG"].templates["DMA"].fields[0];
    let pointer = field
        .pointer
        .as_ref()
        .expect("the field has a pointer fact");
    assert_eq!(pointer.points_to.as_deref(), Some("RX_DESCRIPTOR"));
    assert_eq!(pointer.sources, ["SRC_USE"]);
    assert_eq!(model.types()["TIMG"].file.interrupt_pairs[0].clear, "ACK");
    Ok(())
}

#[test]
fn full_low_bits_and_buffer_pointers_load() -> TestResult {
    for (bits, encoding, target) in [
        ("31:0", "width = 32, shift = 0", true),
        (
            "19:0",
            "width = 20, shift = 0, low-bits = true, high-field = \"HIGH.PREFIX\"",
            true,
        ),
        ("19:0", "width = 20, shift = 0, low-bits = true", true),
        ("31:0", "width = 32, shift = 0", false),
    ] {
        let fixture = fixture()?;
        fixture
            .edit(
                "types/timg.toml",
                "name = \"ADDRESS\"\nbits = \"19:0\"",
                &format!("name = \"ADDRESS\"\nbits = \"{bits}\""),
            )?
            .edit("types/timg.toml", "width = 20, shift = 2", encoding)?;
        if !target {
            fixture
                .edit("types/timg.toml", "points-to = \"RX_DESCRIPTOR\", ", "")?
                .edit("types/timg.toml", "member-offset = 4, ", "")?;
        }
        fixture.load()?;
    }
    Ok(())
}

#[test]
fn pointer_rules_reject_invalid_facts_in_the_type_file() -> TestResult {
    for (from, to) in [
        ("points-to = \"RX_DESCRIPTOR\"", "points-to = \"MISSING\""),
        ("placement = \"dma-window\"", "placement = \"missing\""),
        ("width = 20, shift = 2", "width = 19, shift = 2"),
        ("width = 20, shift = 2", "width = 20, shift = 32"),
        ("width = 20, shift = 2", "width = 20, shift = 13"),
        (
            "width = 20, shift = 2",
            "width = 20, shift = 2, low-bits = true",
        ),
        (
            "width = 20, shift = 2",
            "width = 20, shift = 2, high-field = \"HIGH.PREFIX\"",
        ),
        (
            "width = 20, shift = 2",
            "width = 20, shift = 0, low-bits = true, high-field = \"MISSING.FIELD\"",
        ),
        ("member-offset = 4", "member-offset = 1"),
        ("member-offset = 4", "member-offset = 8"),
        ("points-to = \"RX_DESCRIPTOR\", ", ""),
        (
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"",
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"write-trigger\"",
        ),
        (
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"",
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"\nwrite-range = { min = 0, max = 1 }",
        ),
        // A pointer has no value domain of any form.
        (
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"",
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"\nvalues = [{ name = \"NULL\", value = 0 }]",
        ),
        (
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"",
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"\nvalue-range = { min = 0, max = 1, sources = [\"SRC_HEADER\"] }",
        ),
        (
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"",
            "name = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"\nvalues = [{ name = \"NULL\", value = 0 }]\nwrite-values-only = true",
        ),
        // A high field is no pointer itself and has no domain of its own.
        (
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"plain\"",
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"plain\"\npointer = { placement = \"dma-window\", encoding = { width = 12, shift = 20 }, sources = [\"SRC_USE\"] }",
        ),
        (
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"plain\"",
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"plain\"\nwrite-range = { min = 0, max = 0xfff }",
        ),
    ] {
        let high = "width = 20, shift = 0, low-bits = true, high-field = \"HIGH.PREFIX\"";
        let fixture = fixture()?;
        fixture.edit("types/timg.toml", from, to)?;
        if from.contains("PREFIX") {
            fixture
                .edit("types/timg.toml", "width = 20, shift = 2", high)?
                .edit("types/timg.toml", "member-offset = 4, ", "")?;
        }
        let error = fixture.rejection()?;
        assert!(
            matches!(error.kind, ErrorKind::Pointer(_)),
            "{from} -> {to}: {error}"
        );
        assert!(error.path.ends_with("types/timg.toml"));
        assert_eq!(error.entity, "TIMG.DMA.ADDRESS");
    }
    Ok(())
}

#[test]
fn pointer_facts_cite_known_sources_and_target_word_layouts() -> TestResult {
    let fresh = fixture;
    let fixture = fresh()?;
    fixture.edit(
        "types/timg.toml",
        "member-offset = 4, encoding = { width = 20, shift = 2 }, sources = [\"SRC_USE\"]",
        "member-offset = 4, encoding = { width = 20, shift = 2 }, sources = [\"MISSING\"]",
    )?;
    assert!(matches!(
        fixture.rejection()?.kind,
        ErrorKind::UnknownSource(_)
    ));
    // A byte layout is no target of a pointer.
    let fixture = fresh()?;
    fixture.write(
        "layouts/rx-descriptor.toml",
        "schema = 1\n\n[layout]\nname = \"RX_DESCRIPTOR\"\ndescription = \"Bytes.\"\nsize = 8\nalign = 4\n\n[layout.review]\nsources = [\"SRC_HEADER\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"\n\n[[layout.bytes]]\nname = \"HEAD\"\noffset = 0\nsize = 1\naccess = \"owned\"\ndescription = \"Bytes.\"\n\n[[layout.bytes.fields]]\nname = \"VALUE\"\nbits = \"7:0\"\nwriter = \"cpu\"\ndescription = \"Bytes.\"\n",
    )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Pointer(_)), "{error}");
    // An alignment that loses the encoding's omitted low bits.
    let fixture = fresh()?;
    fixture
        .edit("types/timg.toml", "member-offset = 4", "member-offset = 0")?
        .write(
            "layouts/rx-descriptor.toml",
            &LAYOUT.replace("align = 4", "align = 2"),
        )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::Pointer(_) | ErrorKind::Layouts(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn high_fields_supply_one_prefix() -> TestResult {
    let fresh = fixture;
    let low = "width = 20, shift = 0, low-bits = true, high-field = \"HIGH.PREFIX\"";
    let second = "[[registers]]\nname = \"DMA2\"\noffset = 0xd0\n\n[[registers.fields]]\nname = \"ADDRESS\"\nbits = \"19:0\"\nclass = \"plain\"\npointer = { placement = \"REGION\", encoding = { width = 20, shift = 0, low-bits = true, high-field = \"HIGH.PREFIX\" }, sources = [\"SRC_USE\"] }\n\n[[interrupt-pairs]]";
    let fixture = fresh()?;
    fixture
        .edit("types/timg.toml", "width = 20, shift = 2", low)?
        .edit("types/timg.toml", "member-offset = 4, ", "")?
        .edit(
            "types/timg.toml",
            "[[interrupt-pairs]]",
            &second.replace("REGION", "dma-window"),
        )?;
    fixture.load()?;
    // A second pointer through the same high field from another prefix.
    let fixture = fresh()?;
    fixture
        .edit("types/timg.toml", "width = 20, shift = 2", low)?
        .edit("types/timg.toml", "member-offset = 4, ", "")?
        .edit(
            "types/timg.toml",
            "[[interrupt-pairs]]",
            &second.replace("REGION", "far-ram"),
        )?
        .edit(
            "root.toml",
            "[[ram-regions]]",
            "[[ram-regions]]\nname = \"far-ram\"\nbase = 0x50000000\nsize = 0x1000\nsources = [\"SRC_HEADER\"]\n\n[[ram-regions]]",
        )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Pointer(_)), "{error}");
    assert_eq!(error.entity, "TIMG.DMA2.ADDRESS");
    Ok(())
}

#[test]
fn pointers_require_placed_aligned_targets_and_one_address_prefix() -> TestResult {
    for (file, from, to) in [
        (
            "root.toml",
            "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
            "",
        ),
        (
            "root.toml",
            "base = 0x40800000\nsize = 0x10000",
            "base = 0x40bffff0\nsize = 0x10000",
        ),
        ("root.toml", "base = 0x40800000", "base = 0x140800000"),
    ] {
        let fixture = fixture()?;
        fixture.edit(file, from, to)?;
        assert!(
            matches!(fixture.rejection()?.kind, ErrorKind::Pointer(_)),
            "{file}: {to}"
        );
    }
    let fixture = fixture()?;
    fixture
        .edit(
            "types/timg.toml",
            "width = 20, shift = 2",
            "width = 20, shift = 3",
        )?
        .edit("types/timg.toml", "member-offset = 4", "member-offset = 0")?;
    assert!(matches!(fixture.rejection()?.kind, ErrorKind::Pointer(_)));
    Ok(())
}

#[test]
fn low_bits_require_a_matching_high_field() -> TestResult {
    for (from, to) in [
        (
            "name = \"PREFIX\"\nbits = \"31:20\"",
            "name = \"PREFIX\"\nbits = \"30:20\"",
        ),
        (
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"plain\"",
            "name = \"PREFIX\"\nbits = \"31:20\"\nclass = \"read-only\"",
        ),
    ] {
        let fixture = fixture()?;
        fixture
            .edit(
                "types/timg.toml",
                "width = 20, shift = 2",
                "width = 20, shift = 0, low-bits = true, high-field = \"HIGH.PREFIX\"",
            )?
            .edit("types/timg.toml", from, to)?;
        assert!(matches!(fixture.rejection()?.kind, ErrorKind::Pointer(_)));
    }
    Ok(())
}

#[test]
fn complete_domains_fit_the_field_and_cite_known_sources() -> TestResult {
    for (from, to, sources_error) in [
        ("min = 0, max = 15", "min = 16, max = 15", false),
        ("min = 0, max = 15", "min = 0, max = 16", false),
        (
            "name = \"RUN\", value = 3",
            "name = \"RUN\", value = 4",
            false,
        ),
        // Named values outside the field's value-range.
        (
            "value-range = { min = 0, max = 15, sources = [\"SRC_HEADER\"] }",
            "value-range = { min = 2, max = 15, sources = [\"SRC_HEADER\"] }\nvalues = [{ name = \"LOW\", value = 1 }]",
            false,
        ),
        (
            "max = 15, sources = [\"SRC_HEADER\"]",
            "max = 15, sources = []",
            true,
        ),
        (
            "max = 15, sources = [\"SRC_HEADER\"]",
            "max = 15, sources = [\"MISSING\"]",
            true,
        ),
        (
            "name = \"RUN\", value = 3, sources = [\"SRC_USE\"]",
            "name = \"RUN\", value = 3, sources = [\"MISSING\"]",
            true,
        ),
    ] {
        let fixture = fixture()?;
        fixture.edit("types/timg.toml", from, to)?;
        let error = fixture.rejection()?;
        if sources_error {
            assert!(
                matches!(
                    error.kind,
                    ErrorKind::InvalidSources | ErrorKind::UnknownSource(_)
                ),
                "{error}"
            );
        } else {
            assert!(matches!(error.kind, ErrorKind::Values(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn interrupt_pairs_require_matching_sourced_status_and_clear_words() -> TestResult {
    for (from, to) in [
        ("status = \"INT_RAW\"", "status = \"MISSING\""),
        ("clear = \"ACK\"", "clear = \"MISSING\""),
        ("clear = \"ACK\"", "clear = \"REGCLK_%s\""),
        ("status = \"INT_RAW\"", "status = \"T%s.LOAD\""),
        (
            "name = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[interrupt-pairs]]",
            "name = \"T1\"\nbits = \"2\"\nclass = \"write-one-to-clear\"\n\n[[interrupt-pairs]]",
        ),
        // A status a read changes.
        (
            "name = \"INT_RAW\"\noffset = 0x70\n",
            "name = \"INT_RAW\"\noffset = 0x70\nread-side-effect = { sources = [\"SRC_HEADER\"] }\n",
        ),
        (
            "name = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[registers]]\nname = \"WORD_0080_OPAQUE\"",
            "name = \"T0\"\nbits = \"0\"\nclass = \"read-clears\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[registers]]\nname = \"WORD_0080_OPAQUE\"",
        ),
        // A clear word that is no acknowledgement.
        (
            "name = \"T0\"\nbits = \"0\"\nclass = \"write-one-to-clear\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[interrupt-pairs]]",
            "name = \"T0\"\nbits = \"0\"\nclass = \"plain\"\n\n[[registers.fields]]\nname = \"T1\"\nbits = \"1\"\nclass = \"write-one-to-clear\"\n\n[[interrupt-pairs]]",
        ),
    ] {
        let fixture = fixture()?;
        fixture.edit("types/timg.toml", from, to)?;
        assert!(
            matches!(fixture.rejection()?.kind, ErrorKind::InterruptPair(_)),
            "{from} -> {to}"
        );
    }
    let unknown = fixture()?;
    unknown.edit(
        "types/timg.toml",
        "clear = \"ACK\"\nsources = [\"SRC_HEADER\"]",
        "clear = \"ACK\"\nsources = [\"MISSING\"]",
    )?;
    assert!(matches!(
        unknown.rejection()?.kind,
        ErrorKind::UnknownSource(_)
    ));
    let fixture = fixture()?;
    fixture.edit("types/timg.toml", "[[interrupt-pairs]]", "[[interrupt-pairs]]\nstatus = \"INT_RAW\"\nclear = \"ACK\"\nsources = [\"SRC_HEADER\"]\n\n[[interrupt-pairs]]")?;
    assert!(matches!(
        fixture.rejection()?.kind,
        ErrorKind::InterruptPair(_)
    ));
    Ok(())
}

#[test]
fn pointer_placement_uses_root_region_identity_and_geometry() -> TestResult {
    let fixture = fixture()?;
    fixture.write(
        "types/timg.toml",
        &format!("{TIMG}\n{FACTS}").replace("dma-window", "work-ram"),
    )?;
    fixture
        .edit("root.toml", "name = \"dma-window\"", "name = \"work-ram\"")?
        .edit(
            "root.toml",
            "regions = [\"dma-window\"]",
            "regions = [\"work-ram\"]",
        )?;
    fixture.write(
        "layouts/rx-descriptor.toml",
        &LAYOUT.replace("placement = [\"dma-window\"]\n", ""),
    )?;
    fixture.load()?;
    Ok(())
}
