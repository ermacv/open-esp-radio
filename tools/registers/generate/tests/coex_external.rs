//! Execute the reviewed transaction over an owned raw register, without MMIO.
use std::fs;

#[test]
fn grant_delay_replaces_the_complete_vendor_byte_and_preserves_other_fields() {
    let directory = tempfile::tempdir().expect("transaction fixture directory");
    let path = |name| directory.path().join(name);
    fs::write(
        path("device.toml"),
        r#"schema = 3
chip = "esp32s31"
address-space = "cpu"
fragments = ["coex-external.toml"]
[device]
name = "coex"
version = "1"
description = "External coexistence transaction fixture"
address-unit-bits = 8
width = 32
"#,
    )
    .unwrap();
    fs::write(
        path("coex-external.toml"),
        include_str!("../../../../chips/esp32s31/registers/model/peripherals/coex-external.toml"),
    )
    .unwrap();
    let model = oer_registers_model::RegisterModel::load(&path("device.toml")).unwrap();
    let (svd, _) = model.render_svd().unwrap();
    let policy: toml::Table = toml::from_str(include_str!(
        "../../../../chips/esp32s31/registers/policy/api.toml"
    ))
    .unwrap();
    let named = |table: &str, name: &str| {
        policy[table]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"].as_str() == Some(name))
            .unwrap()
            .clone()
    };
    let mut selected = toml::Table::new();
    selected.insert("schema".into(), policy["schema"].clone());
    selected.insert(
        "bounded-domains".into(),
        toml::Value::Array(vec![named("bounded-domains", "ExternalCoexGrantDelay")]),
    );
    selected.insert(
        "field-replace-modifies".into(),
        toml::Value::Array(vec![named(
            "field-replace-modifies",
            "set_coex_external_grant_delay",
        )]),
    );
    fs::write(path("api.toml"), toml::to_string(&selected).unwrap()).unwrap();
    let policy = oer_registers_model::PacApiPack::load(&path("api.toml")).unwrap();
    let transaction = policy.render_rust(&svd).unwrap();
    // Cargo runs the generated-code fixture in its own directory. The raw
    // register is the real generated type; only its fixed-address peripheral
    // holder is replaced with owned memory, so no hardware address is used.
    let raw =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/pac-raw");
    fs::write(
        path("Cargo.toml"),
        format!(
            r#"[package]
name = "external-grant-delay-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
raw = {{ package = "oer-esp32s31-pac-raw", path = {:?} }}
"#,
            raw
        ),
    )
    .unwrap();
    fs::write(
        path("rust-toolchain.toml"),
        include_str!("../../../../rust-toolchain.toml"),
    )
    .unwrap();
    fs::create_dir(path("src")).unwrap();
    fs::write(
        path("src/lib.rs"),
        format!(
            r#"
pub struct CoexExternal(raw::coex_external::Control);
impl CoexExternal {{
    fn control(&self) -> &raw::coex_external::Control {{ &self.0 }}
}}
{transaction}
#[test]
fn transaction_preserves_neighbours_and_clears_the_low_delay_nibble() {{
    // SOURCE: BLOB_LIBCOEX_EXTERNAL, hal_external_coexist.o::hal_set_extern_coex_delay
    // replaces bits 8:1; ic_set_extern_coex_params supplies delay << 4.
    for (initial, delay, expected) in [
        (0xffff_ffff, 0, 0xffff_fe01),
        (0xffff_fffe, 5, 0xffff_fea0),
        (0x5a5a_5a5a, 15, 0x5a5a_5be0),
        (0xa5a5_a5a5, 0, 0xa5a5_a401),
        (0, 5, 0xa0),
    ] {{
        // SAFETY: Control contains only a volatile u32 cell and PhantomData;
        // zero initializes both validly in this test's owned memory.
        let register = CoexExternal(unsafe {{ core::mem::MaybeUninit::zeroed().assume_init() }});
        // SAFETY: the initial word belongs to this test; no MMIO is involved.
        register.control().modify(|_, writer| unsafe {{ writer.bits(initial) }});
        field_replace_modify::set_coex_external_grant_delay(&register, delay);
        assert_eq!(register.control().read().bits(), expected);
    }}
}}
"#
        ),
    )
    .unwrap();
    let output = oer_os_toolchain::cargo_in(directory.path())
        .args(["test", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", path("target"))
        .output()
        .expect("pinned Cargo transaction fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
