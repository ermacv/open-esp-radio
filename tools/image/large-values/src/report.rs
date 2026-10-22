//! All MIR inspection and layouts use rustc_public.
use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use rustc_public::{
    CrateDef,
    mir::{Operand, TerminatorKind, mono::Instance},
    ty::{Span, Ty},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub struct Configuration {
    pub limit: u64,
    pub output: PathBuf,
    pub manifest: PathBuf,
    pub metadata_hash: String,
    pub driver_id: String,
    pub binary: Option<PathBuf>,
}

impl Configuration {
    pub fn from_arguments(arguments: &[OsString]) -> Option<Self> {
        let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?)
            .join("Cargo.toml")
            .canonicalize()
            .ok()?;
        let workspace: Vec<PathBuf> =
            serde_json::from_str(&std::env::var("OER_LARGE_VALUES_WORKSPACE").ok()?).ok()?;
        let target = std::env::var("OER_LARGE_VALUES_TARGET").ok()?;
        let kind = super::argument(arguments, "--crate-type")?;
        if !workspace.contains(&manifest)
            || kind.split(',').any(|kind| kind == "proc-macro")
            || super::argument(arguments, "--target")? != target
            || !super::argument(arguments, "--emit")?
                .split(',')
                .any(|emit| emit == "link" || emit == "obj")
        {
            return None;
        }
        let output = super::output_path(arguments)?;
        let binary = kind.split(',').any(|kind| kind == "bin").then(|| {
            output.with_file_name(
                output
                    .file_name()
                    .expect("the report has a file name")
                    .to_string_lossy()
                    .trim_end_matches(".large-values.json"),
            )
        });
        Some(Self {
            limit: std::env::var("OER_LARGE_VALUES_LIMIT").ok()?.parse().ok()?,
            output,
            manifest,
            metadata_hash: super::codegen_argument(arguments, "metadata").unwrap_or_default(),
            driver_id: std::env::var("OER_LARGE_VALUES_DRIVER_ID").ok()?,
            binary,
        })
    }
}

#[derive(Serialize)]
pub struct CrateReport {
    schema: u32,
    manifest: PathBuf,
    metadata_hash: String,
    driver_id: String,
    binary_sha256: Option<String>,
    crate_name: String,
    max_move_bytes: u64,
    instances: usize,
    bodies_unavailable: Vec<String>,
    errors: Vec<String>,
    records: Vec<Record>,
}

#[derive(Serialize)]
struct Record {
    instance: String,
    role: String,
    place: String,
    ty: String,
    bytes: u64,
    span: String,
}

impl CrateReport {
    pub fn write(
        &mut self,
        configuration: &Configuration,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(binary) = &configuration.binary {
            let mut file = std::fs::File::open(binary)?;
            let mut digest = Sha256::new();
            std::io::copy(&mut file, &mut digest)?;
            self.binary_sha256 = Some(format!("{:x}", digest.finalize()));
        }
        std::fs::write(&configuration.output, serde_json::to_vec(self)?)?;
        Ok(())
    }
}

pub fn collect(
    instances: &[Instance],
    configuration: &Configuration,
    source_span: &dyn Fn(Span) -> String,
) -> CrateReport {
    let mut report = CrateReport {
        schema: 2,
        manifest: configuration.manifest.clone(),
        metadata_hash: configuration.metadata_hash.clone(),
        driver_id: configuration.driver_id.clone(),
        binary_sha256: None,
        crate_name: rustc_public::local_crate().name,
        max_move_bytes: configuration.limit,
        instances: instances.len(),
        bodies_unavailable: Vec::new(),
        errors: Vec::new(),
        records: Vec::new(),
    };
    let mut sizes = HashMap::new();
    for instance in instances {
        let mut operand_errors = Vec::new();
        let name = instance.name();
        let mut record = |ty: Ty, role: &str, place: String, span: Span| {
            let size = sizes.entry(ty).or_insert_with(|| {
                ty.layout()
                    .map(|layout| layout.shape().size.bytes() as u64)
                    .map_err(|error| format!("{error:?}"))
            });
            match size {
                Ok(bytes) if *bytes > configuration.limit => report.records.push(Record {
                    instance: name.clone(),
                    role: role.into(),
                    place,
                    ty: ty.to_string(),
                    bytes: *bytes,
                    span: source_span(span),
                }),
                Err(error) => report.errors.push(format!("{name} {place} {ty}: {error}")),
                _ => {}
            }
        };
        let Some(body) = instance.body() else {
            // Imported constructors can be synthesized without exported MIR.
            // Their concrete argument/return types are public ABI facts.
            match instance.fn_abi() {
                Ok(abi) => {
                    let span = instance.def.span();
                    record(abi.ret.ty, "return", "signature.return".into(), span);
                    for (index, argument) in abi.args.iter().enumerate() {
                        record(
                            argument.ty,
                            "argument",
                            format!("signature.arg{index}"),
                            span,
                        );
                    }
                }
                Err(error) => report.errors.push(format!("{name} signature: {error:?}")),
            }
            report.bodies_unavailable.push(name);
            continue;
        };
        for (index, declaration) in body.locals().iter().enumerate() {
            let role = if index == 0 {
                "return"
            } else if index <= body.arg_locals().len() {
                "argument"
            } else if body
                .var_debug_info
                .iter()
                .any(|info| info.local() == Some(index))
            {
                "local"
            } else {
                "temporary"
            };
            record(declaration.ty, role, format!("_{index}"), declaration.span);
        }
        for (block, data) in body.blocks.iter().enumerate() {
            if let TerminatorKind::Call { args, .. } = &data.terminator.kind {
                for (index, argument) in args.iter().enumerate() {
                    let role = match argument {
                        Operand::Copy(_) => "call-copy",
                        Operand::Move(_) => "call-move",
                        _ => "call-constant",
                    };
                    match argument.ty(body.locals()) {
                        Ok(ty) => record(
                            ty,
                            role,
                            format!("bb{block}.arg{index}"),
                            data.terminator.span,
                        ),
                        Err(error) => {
                            operand_errors.push(format!("{name} bb{block}.arg{index}: {error:?}"))
                        }
                    }
                }
            }
        }
        report.errors.extend(operand_errors);
    }
    report
        .records
        .sort_by(|a, b| (&a.instance, &a.place).cmp(&(&b.instance, &b.place)));
    report.bodies_unavailable.sort();
    report.errors.sort();
    report.errors.dedup();
    report
}
