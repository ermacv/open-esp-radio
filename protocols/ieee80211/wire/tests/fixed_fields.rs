#![forbid(unsafe_code)]
#![allow(clippy::disallowed_macros, reason = "compiler-test assertions")]

use std::{fs, path::Path, process::Command};

#[test]
fn field_geometry_is_checked_even_for_ordinary_runtime_calls() {
    let scratch = tempfile::tempdir().unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/encoding/fixed.rs");
    let source = scratch.path().join("field.rs");
    for (offset, width, accepted) in [
        (0, 4, true),
        (2, 2, true),
        (4, 0, true),
        (3, 2, false),
        (5, 0, false),
        (usize::MAX, 1, false),
    ] {
        // Include the production helper itself. Calls remain ordinary runtime
        // calls, so rustc must reach monomorphization, not merely cargo check.
        fs::write(&source, format!(
            "include!({helper:?});\nfn main() {{ let mut bytes = [0; 4]; write_array::<{offset}, _, _>(&mut bytes, &[7; {width}]); }}\n"
        )).unwrap();
        let output = Command::new("rustc")
            .args(["--edition=2024", "--crate-name=fixed_field", "--emit=obj"])
            .arg(&source)
            .arg("-o")
            .arg(scratch.path().join("field.o"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            accepted,
            "{offset}/{width}: {stderr}"
        );
        if !accepted {
            assert!(stderr.contains("error[E0080]"), "{stderr}");
            assert!(
                stderr.contains("fixed field exceeds its wire object"),
                "{stderr}"
            );
        }
    }
}
