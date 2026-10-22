//! Compile the pinned headers in an explicit chip configuration. A failed
//! translation unit supplies no contracts. typeof aliases expose resolved
//! type graphs; compiler sizeof constants prove the RV32 register widths.
use super::{
    Options, ast,
    types::{self, Class, Prototype},
};
use crate::Result;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct Signature {
    pub name: String,
    pub arguments: u8,
    pub result: String,
    pub source: String,
    #[serde(skip)]
    pub header: String,
    #[serde(skip)]
    pub sha256: String,
}

#[derive(Debug, Default)]
pub(super) struct Findings {
    pub signatures: BTreeMap<String, Signature>,
    pub unsupported: BTreeMap<String, BTreeSet<String>>,
    pub mentioned: BTreeSet<String>,
    pub headers: BTreeMap<String, String>,
    pub failed_contexts: BTreeMap<String, String>,
    pub conflicts: BTreeMap<String, BTreeSet<String>>,
    pub failed_candidates: BTreeMap<String, BTreeSet<String>>,
}

struct Compiler<'a> {
    options: &'a Options,
    include: Vec<PathBuf>,
    profile: tempfile::TempDir,
    root: PathBuf,
}

fn walk(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.path());
    for e in entries {
        let kind = e.file_type()?;
        if kind.is_dir() {
            walk(&e.path(), out)?;
        } else if kind.is_file() {
            out.push(e.path());
        }
    }
    Ok(())
}

fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|s| {
            s.as_bytes()
                .first()
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        })
}

fn identifier(name: &str) -> bool {
    let mut it = identifiers(name);
    it.next() == Some(name) && it.next().is_none()
}

impl<'a> Compiler<'a> {
    fn new(options: &'a Options) -> Result<(Self, Vec<PathBuf>)> {
        let root = fs::canonicalize(&options.idf)?;
        let profile = tempfile::tempdir()?;
        let config = fs::read(&options.sdkconfig)?;
        let expected = format!(
            "#define CONFIG_IDF_TARGET_{} 1",
            options.chip.to_ascii_uppercase()
        );
        if !String::from_utf8_lossy(&config)
            .lines()
            .any(|l| l == expected)
        {
            return Err(format!("sdkconfig does not select {}", options.chip).into());
        }
        fs::write(profile.path().join("sdkconfig.h"), &config)?;
        let smp = String::from_utf8_lossy(&config)
            .lines()
            .any(|l| l == "#define CONFIG_FREERTOS_SMP 1");
        let excluded_kernel = if smp {
            "FreeRTOS-Kernel"
        } else {
            "FreeRTOS-Kernel-SMP"
        };
        let chip_dirs: BTreeSet<_> = fs::read_dir(root.join("components/soc"))?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("esp32") && name != &options.chip)
            .collect();
        let eligible = |p: &Path| {
            !p.components().any(|c| {
                let c = c.as_os_str().to_string_lossy();
                chip_dirs.contains(c.as_ref())
                    || matches!(
                        c.as_ref(),
                        "test" | "test_apps" | "examples" | "mock" | "mocks" | "linux" | "xtensa"
                    )
                    || c == excluded_kernel
            })
        };
        let mut files = Vec::new();
        walk(&root.join("components"), &mut files)?;
        let headers: Vec<_> = files
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "h") && eligible(p))
            .collect();
        let mut directories = BTreeSet::new();
        for header in &headers {
            let mut parent = header.parent();
            while let Some(p) = parent {
                if p == root {
                    break;
                }
                if p.file_name().is_some_and(|n| {
                    matches!(
                        n.to_str(),
                        Some("include" | "private_include" | "platform_include" | "register")
                    )
                }) || p.file_name().is_some_and(|n| n == options.chip.as_str())
                {
                    directories.insert(p.to_owned());
                }
                parent = p.parent();
            }
        }
        let mut include: Vec<_> = directories.into_iter().collect();
        include.sort_by_key(|p| {
            (
                !p.components()
                    .any(|c| c.as_os_str() == options.chip.as_str()),
                p.clone(),
            )
        });
        include.insert(0, profile.path().to_owned());
        for p in [
            "components/freertos/config/include/freertos".to_owned(),
            format!(
                "components/freertos/{}/portable/riscv/include/freertos",
                if smp {
                    "FreeRTOS-Kernel-SMP"
                } else {
                    "FreeRTOS-Kernel"
                }
            ),
        ] {
            if root.join(&p).is_dir() {
                include.push(root.join(p));
            }
        }
        if let Some(libc) = &options.libc_include {
            include.push(fs::canonicalize(libc)?);
        }
        Ok((
            Self {
                options,
                include,
                profile,
                root,
            },
            headers,
        ))
    }

    fn source(&self, header: &Path, extra: &str) -> Result<String> {
        Ok(format!(
            "#include <sdkconfig.h>\n#include <stddef.h>\n#include <stdint.h>\n#include <stdbool.h>\n#include <esp_err.h>\n{}#include {}\n\
#if !defined(__riscv) || __riscv_xlen != 32\n#error wrong ABI\n#endif\n\
_Static_assert(sizeof(void *) == 4 && sizeof(int) == 4 && sizeof(long) == 4, \"RV32 ILP32 required\");\n{}",
            if header.components().any(|c| c.as_os_str() == "freertos") {
                "#include <freertos/FreeRTOS.h>\n"
            } else {
                ""
            },
            serde_json::to_string(&header.to_string_lossy())?,
            extra
        ))
    }

    fn run(
        &self,
        directory: &Path,
        header: &Path,
        stage: &str,
        extra: &str,
    ) -> Result<std::result::Result<Value, String>> {
        fs::create_dir_all(directory)?;
        let source = directory.join(format!("{stage}.c"));
        fs::write(&source, self.source(header, extra)?)?;
        let ast_path = directory.join(format!("{stage}.json"));
        let mut command = oer_os_process::command(&self.options.clang);
        command.args([
            "--target=riscv32-unknown-elf",
            "-march=rv32im",
            "-mabi=ilp32",
            "-ffreestanding",
            "-std=gnu11",
            "-Wno-everything",
            "-DESP_PLATFORM",
            "-D_GNU_SOURCE",
            "-Werror=implicit-int",
            "-Werror=implicit-function-declaration",
            "-ferror-limit=2",
        ]);
        for path in &self.include {
            command.arg("-I").arg(path);
        }
        let output = command
            .args(["-Xclang", "-ast-dump=json", "-fsyntax-only"])
            .arg(&source)
            .stdout(fs::File::create(&ast_path)?)
            .stderr(Stdio::piped())
            .output()?;
        fs::write(directory.join(format!("{stage}.stderr")), &output.stderr)?;
        if !output.status.success() {
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            let reason = diagnostic
                .lines()
                .find_map(|l| l.split_once("error:").map(|(_, e)| e.trim()))
                .unwrap_or("compiler rejected the header context");
            return Ok(Err(reason.to_owned()));
        }
        Ok(Ok(ast::parse(&fs::read(ast_path)?)?))
    }
}

struct Declaration {
    name: String,
    c_name: String,
    header: String,
    sha256: String,
    line: usize,
}

fn location(node: &Value) -> &Value {
    let loc = &node["loc"];
    loc.get("expansionLoc").unwrap_or(loc)
}

fn declarations(
    root: &Value,
    names: &BTreeSet<String>,
    idf: &Path,
) -> Result<BTreeMap<String, Declaration>> {
    let mut declarations = BTreeMap::new();
    for node in ast::children(root) {
        if node["kind"] != "FunctionDecl"
            || node["isInvalid"] == true
            || node["isImplicit"] == true
            || node["storageClass"] == "static"
        {
            continue;
        }
        let Some(c_name) = node["name"].as_str().filter(|n| identifier(n)) else {
            continue;
        };
        let name = node["mangledName"].as_str().unwrap_or(c_name);
        if !names.contains(name) {
            continue;
        }
        let loc = location(node);
        let (Some(file), Some(offset)) = (loc["actualFile"].as_str(), loc["offset"].as_u64())
        else {
            continue;
        };
        let file = PathBuf::from(file);
        let Ok(relative) = file.strip_prefix(idf) else {
            continue;
        };
        if file.extension().is_none_or(|ext| ext != "h") {
            continue;
        }
        let bytes = fs::read(&file)?;
        let offset = usize::try_from(offset)?;
        if offset >= bytes.len() {
            return Err("compiler source offset exceeds pinned header".into());
        }
        let line = bytes
            .get(..offset)
            .ok_or("invalid source offset")?
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1;
        declarations.insert(
            name.to_owned(),
            Declaration {
                name: name.to_owned(),
                c_name: c_name.to_owned(),
                header: relative.to_string_lossy().replace('\\', "/"),
                sha256: oer_os_durable::sha256_bytes(&bytes),
                line,
            },
        );
    }
    Ok(declarations)
}

fn constants(root: &Value) -> BTreeMap<String, u64> {
    let mut nodes = Vec::new();
    ast::nodes(root, "EnumConstantDecl", &mut nodes);
    nodes
        .into_iter()
        .filter_map(|n| {
            let name = n["name"].as_str()?;
            let mut values = Vec::new();
            ast::nodes(n, "ConstantExpr", &mut values);
            Some((
                name.to_owned(),
                values.first()?["value"].as_str()?.parse().ok()?,
            ))
        })
        .collect()
}

pub(super) fn inspect(
    options: &Options,
    names: &BTreeSet<String>,
    revision: &str,
    output: &Path,
) -> Result<Findings> {
    let (compiler, headers) = Compiler::new(options)?;
    let mut findings = Findings::default();
    let mut selected_headers = 0;
    for header in headers {
        let text = fs::read_to_string(&header)?;
        let hits: BTreeSet<_> = identifiers(&text)
            .filter(|n| names.contains(*n))
            .map(str::to_owned)
            .collect();
        if hits.is_empty() {
            continue;
        }
        selected_headers += 1;
        if selected_headers % 100 == 0 {
            eprintln!(
                "signatures: {selected_headers} header contexts checked for {}",
                options.chip
            );
        }
        findings.mentioned.extend(hits.iter().cloned());
        let relative = header
            .strip_prefix(&compiler.root)?
            .to_string_lossy()
            .to_string();
        let id = oer_os_durable::sha256_bytes(relative.as_bytes());
        let directory = output.join("headers").join(&id[..16]);
        let root = match compiler.run(&directory, &header, "declarations", "")? {
            Ok(v) => v,
            Err(e) => {
                for name in &hits {
                    findings
                        .failed_candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(relative.clone());
                }
                findings.failed_contexts.insert(relative, e);
                continue;
            }
        };
        let declared = declarations(&root, names, &compiler.root)?;
        if declared.is_empty() {
            continue;
        }
        let mut aliases = String::new();
        for (i, d) in declared.values().enumerate() {
            aliases.push_str(&format!(
                "typedef __typeof__({}) __oer_signature_{i};\n",
                d.c_name
            ));
        }
        let root = match compiler.run(&directory, &header, "types", &aliases)? {
            Ok(v) => v,
            Err(e) => {
                for name in &hits {
                    findings
                        .failed_candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(relative.clone());
                }
                findings.failed_contexts.insert(relative, e);
                continue;
            }
        };
        let mut nodes = Vec::new();
        ast::nodes(&root, "TypedefDecl", &mut nodes);
        let aliases_by_name: BTreeMap<_, _> = nodes
            .into_iter()
            .filter_map(|n| Some((n["name"].as_str()?, n)))
            .collect();
        let mut queries = aliases.clone();
        let mut accepted: Vec<(usize, &Declaration, Prototype)> = Vec::new();
        for (i, d) in declared.values().enumerate() {
            let alias_name = format!("__oer_signature_{i}");
            let prototype = aliases_by_name
                .get(alias_name.as_str())
                .ok_or("compiler omitted signature alias")
                .map_err(str::to_owned)
                .and_then(|n| types::prototype(n));
            match prototype {
                Err(reason) => {
                    findings
                        .unsupported
                        .entry(d.name.clone())
                        .or_default()
                        .insert(reason);
                }
                Ok(p) => {
                    queries.push_str(&format!(
                        "enum {{ __oer_size_{i}_0 = {}",
                        if p.result.class == Class::Void {
                            "0".into()
                        } else {
                            p.result.size_expression()
                        }
                    ));
                    for (j, parameter) in p.parameters.iter().enumerate() {
                        queries.push_str(&format!(
                            ", __oer_size_{i}_{} = {}",
                            j + 1,
                            parameter.size_expression()
                        ));
                    }
                    queries.push_str(" };\n");
                    accepted.push((i, d, p));
                }
            }
        }
        if accepted.is_empty() {
            continue;
        }
        let root = match compiler.run(&directory, &header, "sizes", &queries)? {
            Ok(v) => v,
            Err(e) => {
                for name in &hits {
                    findings
                        .failed_candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(relative.clone());
                }
                findings.failed_contexts.insert(relative, e);
                continue;
            }
        };
        let sizes = constants(&root);
        for (i, d, p) in accepted {
            let sizes = (0..=p.parameters.len())
                .map(|j| {
                    sizes
                        .get(&format!("__oer_size_{i}_{j}"))
                        .copied()
                        .ok_or("compiler omitted size proof")
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            match types::contract(&p, &sizes) {
                Err(reason) => {
                    findings
                        .unsupported
                        .entry(d.name.clone())
                        .or_default()
                        .insert(reason);
                }
                Ok((arguments, result)) => {
                    let signature = Signature {
                        name: d.name.clone(),
                        arguments,
                        result: result.into(),
                        source: format!("esp-idf@{revision}/{}:{}", d.header, d.line),
                        header: d.header.clone(),
                        sha256: d.sha256.clone(),
                    };
                    if let Some(previous) = findings.signatures.get(&d.name) {
                        if previous.arguments != arguments || previous.result != result {
                            findings
                                .conflicts
                                .entry(d.name.clone())
                                .or_default()
                                .extend([previous.source.clone(), signature.source.clone()]);
                        }
                    } else {
                        findings.signatures.insert(d.name.clone(), signature);
                    }
                    findings.headers.insert(d.header.clone(), d.sha256.clone());
                }
            }
        }
    }
    for (name, reasons) in &findings.unsupported {
        if let Some(signature) = findings.signatures.get(name) {
            findings
                .conflicts
                .entry(name.clone())
                .or_default()
                .insert(signature.source.clone());
            findings.conflicts.entry(name.clone()).or_default().extend(
                reasons
                    .iter()
                    .map(|r| format!("unsupported declaration: {r}")),
            );
        }
    }
    // Keep the profile alive until all compiler subprocesses finish.
    drop(compiler.profile);
    Ok(findings)
}
