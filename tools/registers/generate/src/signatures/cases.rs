//! The header cases the signature tests inspect, as data: each case's files
//! under a synthetic ESP-IDF root and the names it asks for. The tests read
//! each case's recorded compiler trees (`fixtures/`); `cargo registers
//! signature-fixtures` records them again with a real compiler.
use super::Options;
use crate::Result;
use std::{fs, path::Path};

/// One case: the files beside the common ones and the names inspected.
pub(super) struct Case {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    pub names: &'static [&'static str],
}

const API: &str = "components/api/include/api.h";

pub(super) const CASES: [Case; 7] = [
    Case {
        name: "compiler_resolves_typedefs_pointers_and_rv32_wide_argument_words",
        files: &[(
            API,
            "typedef unsigned long long wide_t;\nvoid empty(void);\nesp_err_t scalar(unsigned char value, const void *address);\nwide_t wide(void *p, wide_t value);\nvoid many(int a,int b,int c,int d,int e,int f,int g,wide_t h);\nvoid (*callback(void))(int);\nvoid adjusted(unsigned char bytes[6], int callback(int));\n",
        )],
        names: &["empty", "scalar", "wide", "many", "callback", "adjusted"],
    },
    Case {
        name: "unsupported_abis_are_reported_and_never_become_contracts",
        files: &[(
            API,
            "struct pair { int a,b; };\nstruct pair aggregate(struct pair value);\nfloat floating(double value);\nvoid variadic(int first,...);\nint old_style();\nvoid supported(int value);\n",
        )],
        names: &[
            "aggregate",
            "floating",
            "variadic",
            "old_style",
            "supported",
        ],
    },
    Case {
        name: "source_locations_survive_includes_and_macro_expansions",
        files: &[
            (
                API,
                "#include \"dependency.h\"\n#define DECLARE(name) void name(unsigned value)\nDECLARE(expanded);\nvoid after(void);\n",
            ),
            (
                "components/api/include/dependency.h",
                "typedef unsigned short small;\nvoid dependency(small value);\n",
            ),
        ],
        names: &["dependency", "expanded", "after"],
    },
    Case {
        name: "chip_configuration_selects_the_declaration_and_other_chip_headers_are_excluded",
        files: &[
            (
                API,
                "#if CONFIG_IDF_TARGET_ESP32S31\nvoid selected(int value);\n#else\nvoid selected(void);\n#endif\n",
            ),
            (
                "components/soc/esp32c5/include/other.h",
                "void selected(void);\n",
            ),
        ],
        names: &["selected"],
    },
    Case {
        name: "conflicting_header_contracts_are_not_silently_resolved",
        files: &[
            (API, "void conflicting(void);\n"),
            (
                "components/api/include/other.h",
                "void conflicting(int value);\n",
            ),
        ],
        names: &["conflicting"],
    },
    Case {
        name: "rejected_header_context_supplies_no_recovery_based_facts",
        files: &[(
            API,
            "missing_type invalid(missing_type value);\nvoid apparently_valid(void);\n",
        )],
        names: &["invalid", "apparently_valid"],
    },
    Case {
        name: "enum_widths_come_from_the_compiler_and_unsupported_redeclarations_conflict",
        files: &[
            (
                API,
                "enum small { SMALL = 1 };\nenum wide { WIDE = 0x100000000ULL };\nenum wide enumeration(enum small a, enum wide b);\nvoid conflicting(int a);\n",
            ),
            (
                "components/api/include/other.h",
                "void conflicting(float a);\n",
            ),
        ],
        names: &["enumeration", "conflicting"],
    },
];

/// The case named `name`.
#[cfg(test)]
pub(super) fn case(name: &str) -> Option<&'static Case> {
    CASES.iter().find(|case| case.name == name)
}

/// Write `case`'s synthetic tree under `root`: the common error type, the
/// chip's sdkconfig and SoC directory, then the case's files.
pub(super) fn materialize(case: &Case, root: &Path, clang: &std::ffi::OsStr) -> Result<Options> {
    let put = |path: &str, content: &str| -> Result<()> {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    };
    fs::create_dir_all(root.join("components/soc/esp32s31"))?;
    put(
        "components/common/include/esp_err.h",
        "typedef int esp_err_t;\n",
    )?;
    put("sdkconfig.h", "#define CONFIG_IDF_TARGET_ESP32S31 1\n")?;
    for (path, content) in case.files {
        put(path, content)?;
    }
    Ok(Options {
        chip: "esp32s31".into(),
        idf: root.to_owned(),
        sdkconfig: root.join("sdkconfig.h"),
        libc_include: None,
        clang: clang.to_owned(),
        check: false,
        output: None,
    })
}
