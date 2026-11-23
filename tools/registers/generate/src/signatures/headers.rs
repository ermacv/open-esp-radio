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
        let mut command = oer_tools_os_process::command(&self.options.clang);
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
            return Ok(Err(refusal(&output.stderr)));
        }
        Ok(Ok(ast::parse(&fs::read(ast_path)?)?))
    }
}

/// The reason a refused header context gives: the compiler's first error.
pub(super) fn refusal(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .find_map(|l| l.split_once("error:").map(|(_, e)| e.trim().to_owned()))
        .unwrap_or_else(|| "compiler rejected the header context".to_owned())
}

/// The pinned tree's root and its headers eligible for `options`' chip,
/// as the compiler would be given them.
#[cfg(test)]
pub(super) fn selection(options: &Options) -> Result<(PathBuf, Vec<PathBuf>)> {
    let (compiler, headers) = Compiler::new(options)?;
    Ok((compiler.root, headers))
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
                sha256: oer_tools_os_durable::sha256_bytes(&bytes),
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

/// A compiler stage one header's inspection needs next: the header, where
/// its outputs go and the declarations appended to its translation unit.
pub(super) struct Request {
    pub header: PathBuf,
    pub directory: PathBuf,
    pub stage: &'static str,
    pub extra: String,
}

/// The declarations whose prototypes the types stage accepted, each with
/// its alias index.
type Accepted = Vec<(usize, Declaration, Prototype)>;

/// What one header's inspection waits for.
enum Stage {
    /// The declarations of the names the header mentions.
    Declarations,
    /// The resolved type of each declaration found, by its alias.
    Types(Vec<Declaration>),
    /// The size of each accepted prototype's result and parameters, after
    /// the types stage's aliases.
    Sizes(String, Accepted),
}

struct Current {
    header: PathBuf,
    relative: String,
    directory: PathBuf,
    hits: BTreeSet<String>,
    stage: Stage,
}

/// The inspection of every eligible header, one compiler stage at a time:
/// [`Inspection::request`] names the stage to run, [`Inspection::advance`]
/// takes its tree (or the compiler's refusal) and decides. Only
/// [`inspect`] runs the compiler.
pub(super) struct Inspection<'a> {
    names: &'a BTreeSet<String>,
    revision: &'a str,
    output: PathBuf,
    root: PathBuf,
    headers: std::vec::IntoIter<PathBuf>,
    selected_headers: usize,
    chip: &'a str,
    current: Option<Current>,
    findings: Findings,
}

impl<'a> Inspection<'a> {
    /// The inspection of `headers` below the pinned tree `root`.
    pub(super) fn new(
        chip: &'a str,
        root: PathBuf,
        headers: Vec<PathBuf>,
        names: &'a BTreeSet<String>,
        revision: &'a str,
        output: &Path,
    ) -> Self {
        Self {
            names,
            revision,
            output: output.to_owned(),
            root,
            headers: headers.into_iter(),
            selected_headers: 0,
            chip,
            current: None,
            findings: Findings::default(),
        }
    }

    /// The next compiler stage to run, or `None` once every header is done.
    pub(super) fn request(&mut self) -> Result<Option<Request>> {
        if self.current.is_none() {
            loop {
                let Some(header) = self.headers.next() else {
                    return Ok(None);
                };
                let text = fs::read_to_string(&header)?;
                let hits: BTreeSet<_> = identifiers(&text)
                    .filter(|n| self.names.contains(*n))
                    .map(str::to_owned)
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                self.selected_headers += 1;
                if self.selected_headers.is_multiple_of(100) {
                    eprintln!(
                        "signatures: {} header contexts checked for {}",
                        self.selected_headers, self.chip
                    );
                }
                self.findings.mentioned.extend(hits.iter().cloned());
                let relative = header
                    .strip_prefix(&self.root)?
                    .to_string_lossy()
                    .to_string();
                let id = oer_tools_os_durable::sha256_bytes(relative.as_bytes());
                let directory = self.output.join("headers").join(&id[..16]);
                self.current = Some(Current {
                    header,
                    relative,
                    directory,
                    hits,
                    stage: Stage::Declarations,
                });
                break;
            }
        }
        let Some(current) = &self.current else {
            return Ok(None);
        };
        let (stage, extra) = match &current.stage {
            Stage::Declarations => ("declarations", String::new()),
            Stage::Types(declared) => ("types", aliases(declared)),
            Stage::Sizes(aliases, accepted) => ("sizes", size_queries(aliases, accepted)),
        };
        Ok(Some(Request {
            header: current.header.clone(),
            directory: current.directory.clone(),
            stage,
            extra,
        }))
    }

    /// Take the tree of the stage [`Self::request`] named, or the
    /// compiler's refusal of the header's context.
    pub(super) fn advance(&mut self, tree: std::result::Result<Value, String>) -> Result<()> {
        let Some(current) = self.current.take() else {
            return Err("no compiler stage is pending".into());
        };
        let root = match tree {
            Ok(root) => root,
            Err(reason) => {
                for name in &current.hits {
                    self.findings
                        .failed_candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(current.relative.clone());
                }
                self.findings
                    .failed_contexts
                    .insert(current.relative, reason);
                return Ok(());
            }
        };
        let next = match current.stage {
            Stage::Declarations => {
                let declared: Vec<_> = declarations(&root, self.names, &self.root)?
                    .into_values()
                    .collect();
                (!declared.is_empty()).then_some(Stage::Types(declared))
            }
            Stage::Types(declared) => {
                let aliases = aliases(&declared);
                let accepted = self.prototypes(&root, declared);
                (!accepted.is_empty()).then_some(Stage::Sizes(aliases, accepted))
            }
            Stage::Sizes(_, accepted) => {
                self.contracts(&root, accepted)?;
                None
            }
        };
        if let Some(stage) = next {
            self.current = Some(Current { stage, ..current });
        }
        Ok(())
    }

    /// Each declaration's prototype from the types stage; one the ABI does
    /// not support is recorded and left out.
    fn prototypes(&mut self, root: &Value, declared: Vec<Declaration>) -> Accepted {
        let mut nodes = Vec::new();
        ast::nodes(root, "TypedefDecl", &mut nodes);
        let aliases_by_name: BTreeMap<_, _> = nodes
            .into_iter()
            .filter_map(|n| Some((n["name"].as_str()?, n)))
            .collect();
        let mut accepted = Vec::new();
        for (i, d) in declared.into_iter().enumerate() {
            let alias_name = format!("__oer_signature_{i}");
            let prototype = aliases_by_name
                .get(alias_name.as_str())
                .ok_or("compiler omitted signature alias")
                .map_err(str::to_owned)
                .and_then(|n| types::prototype(n));
            match prototype {
                Err(reason) => {
                    self.findings
                        .unsupported
                        .entry(d.name.clone())
                        .or_default()
                        .insert(reason);
                }
                Ok(p) => accepted.push((i, d, p)),
            }
        }
        accepted
    }

    /// Each accepted prototype's contract from the sizes stage.
    fn contracts(&mut self, root: &Value, accepted: Accepted) -> Result<()> {
        let sizes = constants(root);
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
                    self.findings
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
                        source: format!("esp-idf@{}/{}:{}", self.revision, d.header, d.line),
                        header: d.header.clone(),
                        sha256: d.sha256.clone(),
                    };
                    if let Some(previous) = self.findings.signatures.get(&d.name) {
                        if previous.arguments != arguments || previous.result != result {
                            self.findings
                                .conflicts
                                .entry(d.name.clone())
                                .or_default()
                                .extend([previous.source.clone(), signature.source.clone()]);
                        }
                    } else {
                        self.findings.signatures.insert(d.name.clone(), signature);
                    }
                    self.findings
                        .headers
                        .insert(d.header.clone(), d.sha256.clone());
                }
            }
        }
        Ok(())
    }

    /// The findings, a name both contracted and unsupported in conflict.
    pub(super) fn finish(mut self) -> Findings {
        let findings = &mut self.findings;
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
        self.findings
    }
}

/// The typeof alias of each declaration, in order.
fn aliases(declared: &[Declaration]) -> String {
    let mut aliases = String::new();
    for (i, d) in declared.iter().enumerate() {
        aliases.push_str(&format!(
            "typedef __typeof__({}) __oer_signature_{i};\n",
            d.c_name
        ));
    }
    aliases
}

/// The types stage's aliases, then an enum of each accepted prototype's
/// sizes.
fn size_queries(aliases: &str, accepted: &[(usize, Declaration, Prototype)]) -> String {
    let mut queries = aliases.to_owned();
    for (i, _, p) in accepted {
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
    }
    queries
}

/// Inspect every header naming one of `names` with the compiler: the one
/// place the compiler runs.
pub(super) fn inspect(
    options: &Options,
    names: &BTreeSet<String>,
    revision: &str,
    output: &Path,
) -> Result<Findings> {
    let (compiler, headers) = Compiler::new(options)?;
    let mut inspection = Inspection::new(
        &options.chip,
        compiler.root.clone(),
        headers,
        names,
        revision,
        output,
    );
    while let Some(request) = inspection.request()? {
        let tree = compiler.run(
            &request.directory,
            &request.header,
            request.stage,
            &request.extra,
        )?;
        inspection.advance(tree)?;
    }
    // Keep the profile alive until all compiler subprocesses finish.
    drop(compiler.profile);
    Ok(inspection.finish())
}

/// One recorded compiler stage of a case: the text appended to the unit,
/// the hash of the header it compiled, and its reduced tree or refusal.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct RecordedStage {
    #[serde(rename = "//")]
    pub note: String,
    pub header_sha256: String,
    pub extra: String,
    #[serde(default)]
    pub tree: Option<Value>,
    #[serde(default)]
    pub refusal: Option<String>,
}

/// The file of `relative`'s `stage` in a case's fixture directory.
pub(super) fn recorded_path(directory: &Path, relative: &str, stage: &str) -> PathBuf {
    directory.join(format!("{}.{stage}.json", relative.replace('/', "__")))
}

/// The nodes of `tree` the `stage` decides on, each location carrying its
/// file (no cursor across removed nodes) and the tree's `root` as `/idf`.
fn reduce(tree: &Value, stage: &str, names: &BTreeSet<String>, root: &Path) -> Result<Value> {
    fn locate(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if let Some(file) = map.remove("actualFile") {
                    map.insert("file".into(), file);
                }
                // Node identities are the compiler's addresses, new at each
                // run; nothing decides on them.
                map.retain(|key, _| key != "id" && !key.ends_with("Id") && key != "previousDecl");
                for child in map.values_mut() {
                    locate(child);
                }
            }
            Value::Array(items) => items.iter_mut().for_each(locate),
            _ => {}
        }
    }
    let kept: Vec<Value> = ast::children(tree)
        .iter()
        .filter(|node| match stage {
            "declarations" => {
                node["kind"] == "FunctionDecl"
                    && node["name"].as_str().is_some_and(|n| names.contains(n))
            }
            "types" => {
                node["kind"] == "TypedefDecl"
                    && node["name"]
                        .as_str()
                        .is_some_and(|n| n.starts_with("__oer_signature_"))
            }
            _ => {
                node["kind"] == "EnumDecl"
                    && ast::children(node).iter().any(|c| {
                        c["name"]
                            .as_str()
                            .is_some_and(|n| n.starts_with("__oer_size_"))
                    })
            }
        })
        .cloned()
        .map(|mut node| {
            locate(&mut node);
            node
        })
        .collect();
    let text = serde_json::to_string(
        &serde_json::json!({ "kind": "TranslationUnitDecl", "inner": kept }),
    )?
    .replace(&*root.to_string_lossy(), "/idf");
    Ok(serde_json::from_str(&text)?)
}

/// Record every case's compiler stages with `clang` under `directory`
/// (`<case>/<header>.<stage>.json`): the one place a case meets a compiler.
pub(super) fn record_cases(clang: &std::ffi::OsStr, directory: &Path) -> Result<()> {
    let version = String::from_utf8_lossy(
        &oer_tools_os_process::command(clang)
            .arg("--version")
            .output()?
            .stdout,
    )
    .lines()
    .next()
    .unwrap_or("clang")
    .to_owned();
    for case in &super::cases::CASES {
        let scratch = tempfile::tempdir()?;
        let options = super::cases::materialize(case, scratch.path(), clang)?;
        let names: BTreeSet<String> = case.names.iter().map(|s| (*s).to_owned()).collect();
        let (compiler, headers) = Compiler::new(&options)?;
        let output = scratch.path().join("output");
        let mut inspection = Inspection::new(
            &options.chip,
            compiler.root.clone(),
            headers,
            &names,
            "synthetic-pin",
            &output,
        );
        let fixtures = directory.join(case.name);
        match fs::remove_dir_all(&fixtures) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::create_dir_all(&fixtures)?;
        while let Some(request) = inspection.request()? {
            let relative = request
                .header
                .strip_prefix(&compiler.root)?
                .to_string_lossy()
                .into_owned();
            let tree = compiler.run(
                &request.directory,
                &request.header,
                request.stage,
                &request.extra,
            )?;
            let recorded = RecordedStage {
                note: format!(
                    "{version}: {relative}, {} stage; the nodes the stage decides on",
                    request.stage
                ),
                header_sha256: oer_tools_os_durable::sha256_file(&request.header)?,
                extra: request.extra.clone(),
                tree: match &tree {
                    Ok(tree) => Some(reduce(tree, request.stage, &names, &compiler.root)?),
                    Err(_) => None,
                },
                refusal: tree.as_ref().err().cloned(),
            };
            fs::write(
                recorded_path(&fixtures, &relative, request.stage),
                serde_json::to_string_pretty(&recorded)? + "\n",
            )?;
            inspection.advance(tree)?;
        }
    }
    Ok(())
}
