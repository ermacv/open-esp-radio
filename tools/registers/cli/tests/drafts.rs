//! Synthetic source-authoring lifecycle; no generated hardware constants are asserted.
use oer_registers_model::RegisterModel;
use std::{fs, path::Path};

const INIT: &str = r#"
schema = 1
chip = "fixture-chip"
address-space = "cpu"
[device]
name = "FIXTURE"
version = "1"
description = "Synthetic register lifecycle"
address-unit-bits = 8
width = 32
[[peripherals]]
name = "CONTROL"
base = 0x20000
length = 256
"#;
const SVD: &str = r#"<?xml version="1.0"?>
<device schemaVersion="1.3"><name>FIXTURE</name><version>1</version>
<description>Synthetic declaration</description><addressUnitBits>8</addressUnitBits>
<width>32</width><size>32</size><peripherals><peripheral><name>CONTROL</name>
<baseAddress>0x20000</baseAddress><registers><register><name>STATUS</name>
<addressOffset>0</addressOffset><access>read-only</access><fields><field>
<name>READY</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field></fields>
</register></registers></peripheral></peripherals><vendorExtensions><private>retained</private>
</vendorExtensions></device>"#;

#[test]
fn invalid_geometry_or_svd_never_leaves_a_draft_or_overwrites_sources() {
    let dir = tempfile::tempdir().unwrap();
    for (index, source) in [
        INIT.replace("length = 256", "length = 0"),
        INIT.replace("schema = 1", "schema = 8"),
        format!("{INIT}\n[[peripherals]]\nname = 'OVERLAP'\nbase = 0x20004\nlength = 8"),
    ]
    .iter()
    .enumerate()
    {
        let input = dir.path().join(format!("{index}.toml"));
        let output = dir.path().join(format!("draft-{index}"));
        fs::write(&input, source).unwrap();
        assert!(oer_registers_generate::initialize_model(&input, &output).is_err());
        assert!(!output.exists());
        assert_eq!(fs::read_to_string(input).unwrap(), *source);
    }
    let input = dir.path().join("broken.svd");
    fs::write(&input, SVD.replace("<bitWidth>1", "<bitWidth>64")).unwrap();
    let output = dir.path().join("invalid-import");
    assert!(oer_registers_generate::import_svd(&input, &output, "fixture-chip", "cpu").is_err());
    assert!(!output.exists());
}

fn write(path: &Path, name: &str, source: &str) {
    fs::write(path.join(name), source).unwrap();
}

#[test]
fn explicit_review_materializes_native_model_and_publishes_all_four_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "init.toml", INIT);
    oer_registers_generate::initialize_model(&root.join("init.toml"), &root.join("model")).unwrap();
    write(
        root,
        "reviewed.toml",
        r#"
schema = 2
id = "fixture-review"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[applies-to]
chips = ["fixture-chip"]
chip-revisions = ["rev0"]
[[assertions]]
id = "control.identity"
subject = "register:fixture-chip/cpu/0x20000/32"
kind = "register-identity"
value = "CONTROL.STATUS"
[[assertions.evidence]]
source = "fixture"
locator = "reviewed synthetic declaration, not inferred access width"
[[assertions]]
id = "control.access"
subject = "register:fixture-chip/cpu/0x20000/32"
kind = "register-access"
value = "read-write"
[[assertions.evidence]]
source = "fixture"
locator = "explicit synthetic access contract"
"#,
    );
    write(root, "api.toml", "schema = 5\n");
    write(root, "lints.toml", "schema = 1\n");
    write(
        root,
        "evidence.toml",
        "schema = 1\n[[sources]]\nid = 'fixture'\ndescription = 'Synthetic source contract'\n",
    );
    write(
        root,
        "ownership.toml",
        "schema = 1\nowned-ranges = ['control']\n",
    );
    write(
        root,
        "memory.toml",
        r#"
schema = 1
default-address-space = "cpu"
[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"
[[regions]]
name = "control"
address-space = "cpu"
kind = "mmio"
start = 0x20000
end-exclusive = 0x20100
permissions = "rw"
volatile = true
"#,
    );
    write(
        root,
        "publication.toml",
        r#"
schema = 1
model = "model/device.toml"
reviewed = ["reviewed.toml"]
memory = "memory.toml"
ownership = "ownership.toml"
api = "api.toml"
lints = "lints.toml"
evidence = ["evidence.toml"]
[applicability]
ecosystems = []
chip = "fixture-chip"
chip-revisions = ["rev0"]
artifact-lineages = []
[outputs]
svd = "radio.svd"
bindings = "bindings.toml"
[outputs.pac]
raw = "raw.rs"
api = "api.rs"
crate-name = "fixture_pac"
target = "none"
edition = "2024"
"#,
    );
    let publication =
        oer_registers_generate::Publication::load(&root.join("publication.toml")).unwrap();
    assert!(publication.generate(true).is_err());
    publication.generate(false).unwrap();
    publication.generate(true).unwrap();
    for name in ["radio.svd", "raw.rs", "api.rs", "bindings.toml"] {
        assert!(!fs::read(root.join(name)).unwrap().is_empty());
    }
    // Accepted overlays do not mutate the draft's unreviewed source geometry.
    assert!(
        RegisterModel::load(&root.join("model/device.toml"))
            .unwrap()
            .register_identities()
            .unwrap()
            .is_empty()
    );
    let source = fs::read_to_string(root.join("publication.toml")).unwrap();
    write(
        root,
        "publication.toml",
        &source.replace("chip = \"fixture-chip\"", "chip = \"other-chip\""),
    );
    assert!(oer_registers_generate::Publication::load(&root.join("publication.toml")).is_err());

    // A publication read only through its SVD and binding index: no PAC, no
    // PAC API policy, and a binding index that names no crate.
    let pac = "api = \"api.toml\"\n";
    let without_pac = source
        .replace(pac, "")
        .replace("svd = \"radio.svd\"", "svd = \"platform.svd\"")
        .replace(
            "bindings = \"bindings.toml\"",
            "bindings = \"platform.bindings.toml\"",
        );
    let without_pac = &without_pac[..without_pac.find("[outputs.pac]").unwrap()];
    write(root, "platform.toml", without_pac);
    let platform = oer_registers_generate::Publication::load(&root.join("platform.toml")).unwrap();
    platform.generate(false).unwrap();
    platform.generate(true).unwrap();
    let index =
        oer_registers_bindings::BindingIndex::load(&root.join("platform.bindings.toml")).unwrap();
    assert_eq!(index.crate_name, None);
    assert!(!index.registers.is_empty());
    // A PAC API policy without a PAC, or a PAC without one, is refused.
    write(root, "platform.toml", &format!("{pac}{without_pac}"));
    assert!(oer_registers_generate::Publication::load(&root.join("platform.toml")).is_err());
    write(root, "platform.toml", &source.replace(pac, ""));
    assert!(oer_registers_generate::Publication::load(&root.join("platform.toml")).is_err());
}
