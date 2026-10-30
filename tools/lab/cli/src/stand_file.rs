//! `cargo stand stand-file convert`: the one rewrite of a schema 1 stand
//! file to schema 2 (`oer_lab_model::convert`). The stand file holds
//! credentials: the result is written private (mode 0600), never printed,
//! and an in-place rewrite keeps the schema 1 file beside it.

use std::path::Path;

use crate::Result;

/// Where an in-place rewrite keeps the schema 1 file.
pub(crate) fn schema_1_copy(path: &Path) -> std::path::PathBuf {
    path.with_extension("toml.schema-1")
}

/// Rewrite the schema 1 stand file at `path` to schema 2 at `output`; when
/// `output` is `path`, the schema 1 file is kept beside it first.
pub(crate) fn convert(path: &Path, output: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read stand file {}: {error}", path.display()))?;
    let converted = oer_lab_model::convert::from_schema_1(&text)?;
    // What it writes is a stand file this build reads.
    oer_lab_model::Stand::parse(&converted.text)?.validate()?;
    if output == path {
        let kept = schema_1_copy(path);
        crate::init::write_private(&kept, &text)?;
        println!("kept the schema 1 file as {}", kept.display());
    }
    crate::init::write_private(output, &converted.text)?;
    for note in &converted.notes {
        println!("{note}");
    }
    println!("wrote the schema 2 stand file {}", output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_in_place_rewrite_keeps_the_schema_1_file_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stand.toml");
        let schema_1 = "schema = 1\n[station_fixture]\nkind = \"open-wrt\"\nradio = \"radio0\"\n\
            ap_section = \"ap\"\nssh_target = \"made-up\"\nwireless_interface = \"phy0-ap0\"\n\
            channel = 13\n";
        std::fs::write(&path, schema_1).unwrap();
        convert(&path, &path).unwrap();
        let kept = schema_1_copy(&path);
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), schema_1);
        for file in [&path, &kept] {
            let mode = std::fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let stand = oer_lab_model::Stand::load(&path).unwrap();
        assert_eq!(stand.router.unwrap().channels, [13]);
    }
}
