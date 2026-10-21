use std::fs;

use oer_registers_contracts::{FactCompleteness, FactProvenance, NameOrigin};

use crate::{RegisterModel, RegisterModelFragment, ReviewAnnotation};

const GEOMETRY: &str = r#"schema = 2
[[peripherals]]
name = "RADIO"
baseAddress = 0x1000
[[peripherals.registers]]
[peripherals.registers.register]
name = "CONTROL%s"
addressOffset = 0
size = 32
dim = 2
dimIncrement = 4
dimIndex = "0,1"
[[peripherals.registers.register.fields]]
name = "ENABLE"
bitOffset = 0
bitWidth = 1
[[peripherals.registers.register.fields]]
name = "BITS_1_31_OPAQUE"
bitOffset = 1
bitWidth = 31
"#;

const DEFAULTS: &str = r#"
[[peripheral-review]]
entity = "RADIO"
opaque-entities = ["RADIO.CONTROL%s.BITS_1_31_OPAQUE"]
[peripheral-review.peripheral]
sources = ["peripheral"]
provenance = "derived"
accuracy = "exact"
completeness = "partial"
[peripheral-review.register]
sources = ["register"]
provenance = "observed"
accuracy = "exact"
completeness = "partial"
naming = "descriptive"
[peripheral-review.field]
sources = ["field"]
provenance = "observed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"
[peripheral-review.opaque]
sources = ["opaque"]
provenance = "derived"
accuracy = "positional"
completeness = "unknown"
naming = "opaque"
"#;

fn load(fragments: &[&str], publication: bool) -> crate::Result<RegisterModel> {
    let dir = tempfile::tempdir().expect("temporary fixture directory");
    let names: Vec<_> = (0..fragments.len())
        .map(|i| format!("fragment-{i}.toml"))
        .collect();
    for (name, text) in names.iter().zip(fragments) {
        fs::write(dir.path().join(name), text).expect("write peripheral fixture");
    }
    let manifest = dir.path().join("device.toml");
    fs::write(
        &manifest,
        format!(
            r#"schema = 3
chip = "fixture-chip"
fragments = {names:?}
[device]
name = "fixture"
version = "1"
description = "fixture"
address-unit-bits = 8
width = 32
"#
        ),
    )
    .expect("write fixture manifest");
    if publication {
        RegisterModel::load_for_publication(&manifest)
    } else {
        RegisterModel::load(&manifest)
    }
}

fn reserved_fixture(annotation: &str) -> String {
    GEOMETRY
        .split("[[peripherals.registers.register.fields]]\nname = \"BITS_1_31_OPAQUE\"")
        .next()
        .unwrap()
        .replace("size = 32", &format!("size = 32\nreserved = {annotation}"))
}

#[test]
fn cited_reserved_masks_roundtrip_expand_arrays_and_publish_no_fields() {
    let text = reserved_fixture("{ mask = 0xfffffffe, sources = [\"vendor-header\"] }");
    let fragment: RegisterModelFragment = toml_edit::de::from_str(&text).unwrap();
    let roundtrip = toml_edit::ser::to_string_pretty(&fragment).unwrap();
    let model = load(&[&roundtrip], false).unwrap();
    let geometry = model.register_geometry().unwrap();
    assert_eq!(geometry.len(), 2);
    for register in geometry {
        let reserved = register.reserved.unwrap();
        assert_eq!(reserved.mask, 0xfffffffe);
        assert_eq!(reserved.sources, ["vendor-header"]);
        assert_eq!(register.fields.len(), 1);
        assert_eq!(register.fields[0].name, "ENABLE");
    }
    let (svd, summary) = model.render_svd().unwrap();
    assert!(!svd.contains("reserved"));
    assert!(!svd.contains("BITS_1_31_OPAQUE"));
    assert!(
        summary
            .projection_omissions
            .iter()
            .any(|line| line.contains("2 cited reserved masks"))
    );
    assert_eq!(
        model.reserved_sources().collect::<Vec<_>>(),
        ["vendor-header", "vendor-header"]
    );
}

#[test]
fn reserved_masks_reject_overlap_outside_width_and_missing_or_duplicate_sources() {
    for annotation in [
        "{ mask = 1, sources = [\"header\"] }",
        "{ mask = 0x100000000, sources = [\"header\"] }",
        "{ mask = 0, sources = [\"header\"] }",
        "{ mask = 2, sources = [] }",
        "{ mask = 2, sources = [\"header\", \"header\"] }",
        "{ mask = 2, sources = [\"\"] }",
        "{ mask = 2, sources = [\"header\"], guessed = true }",
    ] {
        assert!(
            load(&[&reserved_fixture(annotation)], false).is_err(),
            "accepted {annotation}"
        );
    }
}

#[test]
fn wide_reserved_masks_keep_high_bits_through_native_roundtrip() {
    let text = reserved_fixture(
        "{ mask = \"0xfffffffffffffffffffffffffffffffe\", sources = [\"header\"] }",
    )
    .replace("size = 32", "size = 128")
    .replace("dimIncrement = 4", "dimIncrement = 16");
    let fragment: RegisterModelFragment = toml_edit::de::from_str(&text).unwrap();
    let text = toml_edit::ser::to_string_pretty(&fragment).unwrap();
    let model = load(&[&text], false).unwrap();
    assert!(
        model
            .register_geometry()
            .unwrap()
            .iter()
            .all(|register| register.reserved.as_ref().unwrap().mask == u128::MAX - 1)
    );
}

#[test]
fn reserved_annotations_on_peripherals_or_fields_are_not_silently_discarded() {
    for text in [
        GEOMETRY.replace(
            "baseAddress = 0x1000",
            "baseAddress = 0x1000\nreserved = { mask = 2, sources = [\"header\"] }",
        ),
        GEOMETRY.replace(
            "bitOffset = 0",
            "reserved = { mask = 2, sources = [\"header\"] }\nbitOffset = 0",
        ),
    ] {
        assert!(
            load(&[&text], false)
                .unwrap_err()
                .to_string()
                .contains("reserved is a register property")
        );
    }
}

fn compact() -> String {
    GEOMETRY.to_owned() + DEFAULTS
}

fn review<'a>(model: &'a RegisterModel, entity: &str) -> &'a ReviewAnnotation {
    model
        .review()
        .iter()
        .find(|review| review.entity == entity)
        .expect("resolved fixture review")
}

fn error(text: &str) -> String {
    load(&[text], true)
        .expect_err("invalid peripheral fixture")
        .to_string()
}

#[test]
fn levels_and_explicit_opaque_entities_resolve_before_every_projection() {
    let model = load(&[&compact()], true).expect("valid peripheral defaults");
    assert_eq!(model.review().len(), 4);
    let peripheral = review(&model, "RADIO");
    assert_eq!(peripheral.sources, ["peripheral"]);
    assert_eq!(peripheral.provenance, Some(FactProvenance::Derived));
    assert_eq!(peripheral.naming, None);
    let register = review(&model, "RADIO.CONTROL%s");
    assert_eq!(register.sources, ["register"]);
    assert_eq!(register.completeness, Some(FactCompleteness::Partial));
    let field = review(&model, "RADIO.CONTROL%s.ENABLE");
    assert_eq!(field.sources, ["field"]);
    assert_eq!(field.completeness, Some(FactCompleteness::Complete));
    let opaque = review(&model, "RADIO.CONTROL%s.BITS_1_31_OPAQUE");
    assert_eq!(opaque.sources, ["opaque"]);
    assert_eq!(opaque.naming, Some(NameOrigin::Opaque));
    let projected = model
        .register_review_annotations()
        .expect("expanded register reviews");
    assert_eq!(projected.len(), 2);
    assert!(projected.values().all(|r| r.sources == ["register"]));
    let (svd, _) = model.render_svd().expect("resolved SVD projection");
    assert!(svd.contains("Project-assigned name."));
    assert!(svd.contains("Opaque: meaning not established."));
    assert!(!svd.contains("peripheral-review"));
    assert!(!svd.contains("opaque-entities"));
}

#[test]
fn explicit_records_replace_defaults_including_omitted_keys() {
    let text = compact()
        + r#"
[[review]]
entity = "RADIO.CONTROL%s.ENABLE"
naming = "vendor"
sources = ["header"]
"#;
    let model = load(&[&text], true).expect("whole-record replacement");
    let explicit = review(&model, "RADIO.CONTROL%s.ENABLE");
    assert_eq!(explicit.sources, ["header"]);
    assert_eq!(explicit.naming, Some(NameOrigin::Vendor));
    assert_eq!(explicit.provenance, None);
    assert_eq!(explicit.accuracy, None);
    assert_eq!(explicit.completeness, None);
    let omitted_naming = text.replace("naming = \"vendor\"", "");
    assert!(error(&omitted_naming).contains("has no reviewed naming"));
}

#[test]
fn opaque_selection_is_explicit_and_never_inferred_from_the_suffix() {
    let unmarked = compact().replace(
        "opaque-entities = [\"RADIO.CONTROL%s.BITS_1_31_OPAQUE\"]",
        "opaque-entities = []",
    );
    assert!(error(&unmarked).contains("not reviewed with naming = \"opaque\""));
    let misnamed = compact().replace("BITS_1_31_OPAQUE", "RESERVED");
    assert!(error(&misnamed).contains("does not end in"));
    let register = compact()
        .replace("CONTROL%s", "WORD_0000_%s_OPAQUE")
        .replace(
            "opaque-entities = [",
            "opaque-entities = [\"RADIO.WORD_0000_%s_OPAQUE\", ",
        );
    let model = load(&[&register], true).expect("opaque register and field");
    assert_eq!(
        review(&model, "RADIO.WORD_0000_%s_OPAQUE").sources,
        ["opaque"]
    );
    assert_eq!(
        review(&model, "RADIO.WORD_0000_%s_OPAQUE.ENABLE").sources,
        ["field"]
    );
    let override_opaque = compact()
        + r#"
[[review]]
entity = "RADIO.CONTROL%s.BITS_1_31_OPAQUE"
naming = "descriptive"
"#;
    assert!(error(&override_opaque).contains("explicitly opaque entity"));
}

#[test]
fn defaults_do_not_leak_to_another_peripheral_in_the_same_or_another_file() {
    let other = GEOMETRY
        .replace("RADIO", "OTHER")
        .replace("0x1000", "0x2000");
    let other_body = other.strip_prefix("schema = 2\n").expect("fixture schema");
    for fragments in [vec![compact() + other_body], vec![compact(), other.clone()]] {
        let refs: Vec<_> = fragments.iter().map(String::as_str).collect();
        assert!(
            load(&refs, true)
                .expect_err("unreviewed second peripheral")
                .to_string()
                .contains("has no reviewed naming")
        );
    }
    let independent = compact()
        + other_body
        + &DEFAULTS
            .replace("RADIO", "OTHER")
            .replace("\"register\"", "\"other-register\"");
    let model = load(&[&independent], true).expect("independent peripheral scopes");
    assert_eq!(
        review(&model, "OTHER.CONTROL%s").sources,
        ["other-register"]
    );
    assert_eq!(review(&model, "RADIO.CONTROL%s").sources, ["register"]);
}

#[test]
fn clusters_and_their_dim_templates_use_the_register_level() {
    let geometry = r#"schema = 2
[[peripherals]]
name = "RADIO"
baseAddress = 0x1000
[[peripherals.registers]]
[peripherals.registers.cluster]
name = "CHANNEL%s"
addressOffset = 0
dim = 2
dimIncrement = 16
dimIndex = "0,1"
[[peripherals.registers.cluster.children]]
[peripherals.registers.cluster.children.register]
name = "CONTROL"
addressOffset = 0
size = 32
[[peripheral-review]]
entity = "RADIO"
[peripheral-review.peripheral]
sources = ["peripheral"]
[peripheral-review.register]
sources = ["register-level"]
"#;
    let model = load(&[geometry], false).expect("cluster register-level defaults");
    assert_eq!(
        review(&model, "RADIO.CHANNEL%s").sources,
        ["register-level"]
    );
    assert_eq!(
        review(&model, "RADIO.CHANNEL%s.CONTROL").sources,
        ["register-level"]
    );
}

#[test]
fn unknown_duplicate_and_redundant_records_and_scopes_are_errors() {
    for (append, reason) in [
        ("[[review]]\nentity = \"RADIO.MISSING\"\n", "does not exist"),
        (
            "[[review]]\nentity = \"RADIO.CONTROL%s\"\nnaming = \"vendor\"\nsources = [\"header\"]\n[[review]]\nentity = \"RADIO.CONTROL%s\"\nnaming = \"descriptive\"\n",
            "duplicate",
        ),
        (
            "[[review]]\nentity = \"RADIO.CONTROL%s.ENABLE\"\nsources = [\"field\"]\nprovenance = \"observed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"\n",
            "repeats its applicable peripheral review default",
        ),
        (
            "[[review]]\nentity = \"RADIO.CONTROL%s.BITS_1_31_OPAQUE\"\nsources = [\"opaque\"]\nprovenance = \"derived\"\naccuracy = \"positional\"\ncompleteness = \"unknown\"\nnaming = \"opaque\"\n",
            "repeats its applicable peripheral review default",
        ),
        (
            "[[peripheral-review]]\nentity = \"RADIO\"\n",
            "duplicate peripheral review scope",
        ),
        (
            "[[peripheral-review]]\nentity = \"ABSENT\"\n",
            "does not exist in this fragment",
        ),
    ] {
        let message = error(&(compact() + append));
        assert!(message.contains(reason), "{reason}: {message}");
    }
    for (from, to, reason) in [
        (
            "[peripheral-review.field]",
            "[peripheral-review.field]\nsource = []",
            "unknown field",
        ),
        (
            "[peripheral-review.opaque]",
            "[peripheral-review.opaque]\naccuraccy = \"exact\"",
            "unknown field",
        ),
        ("opaque-entities =", "opaquee =", "unknown field"),
        (
            "RADIO.CONTROL%s.BITS_1_31_OPAQUE\"]",
            "RADIO.MISSING\"]",
            "not a register or field",
        ),
        (
            "opaque-entities = [",
            "opaque-entities = [\"RADIO.CONTROL%s.BITS_1_31_OPAQUE\", ",
            "duplicate opaque entity",
        ),
        (
            "[peripheral-review.field]",
            "[peripheral-review.fields]",
            "unknown field",
        ),
    ] {
        let message = error(&compact().replace(from, to));
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

#[test]
fn missing_levels_and_invalid_resolved_metadata_are_rejected() {
    let missing = compact().replace("[peripheral-review.field]\nsources = [\"field\"]\nprovenance = \"observed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"\n", "");
    assert!(error(&missing).contains("has no resolved review"));
    for (from, to, reason) in [
        (
            "sources = [\"field\"]",
            "sources = [\"field\", \"field\"]",
            "duplicate sources",
        ),
        (
            "sources = [\"field\"]",
            "sources = [\"\"]",
            "empty or duplicate sources",
        ),
        (
            "accuracy = \"exact\"\n",
            "",
            "must define provenance, accuracy and completeness together",
        ),
    ] {
        let message = error(&compact().replacen(from, to, 1));
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

#[test]
fn serializing_a_loaded_fragment_contains_only_resolved_records_and_svd_geometry() {
    let fragment: RegisterModelFragment =
        toml_edit::de::from_str(&compact()).expect("resolved fragment");
    assert_eq!(fragment.review.len(), 4);
    let serialized =
        toml_edit::ser::to_string_pretty(&fragment).expect("resolved fragment serialization");
    assert!(!serialized.contains("peripheral-review"));
    let expanded = load(&[&serialized], true).expect("expanded reviews");
    let compact = load(&[&compact()], true).expect("compact reviews");
    assert_eq!(
        expanded.render_svd().expect("expanded SVD").0,
        compact.render_svd().expect("compact SVD").0
    );
}
