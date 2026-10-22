//! All MIR inspection and layouts use rustc_public.
use std::{
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use rustc_public::{
    mir::{Operand, TerminatorKind, mono::Instance},
    ty::{Span, Ty},
};
use serde::Serialize;

pub struct Configuration {
    pub limit: u64,
    pub output: PathBuf,
}

impl Configuration {
    pub fn from_arguments(arguments: &[OsString]) -> Option<Self> {
        let target = std::env::var("OER_LARGE_VALUES_TARGET").ok()?;
        if super::argument(arguments, "--target")? != target
            || !super::argument(arguments, "--emit")?
                .split(',')
                .any(|emit| emit == "link" || emit == "obj")
        {
            return None;
        }
        Some(Self {
            limit: std::env::var("OER_LARGE_VALUES_LIMIT").ok()?.parse().ok()?,
            output: super::output_path(arguments)?,
        })
    }
}

#[derive(Serialize)]
pub struct CrateReport {
    schema: u32,
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
    pub fn write(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::write(path, serde_json::to_vec(self)?)?;
        Ok(())
    }
}

pub fn collect(
    instances: &[Instance],
    limit: u64,
    source_span: &dyn Fn(Span) -> String,
) -> CrateReport {
    let mut report = CrateReport {
        schema: 1,
        crate_name: rustc_public::local_crate().name,
        max_move_bytes: limit,
        instances: instances.len(),
        bodies_unavailable: Vec::new(),
        errors: Vec::new(),
        records: Vec::new(),
    };
    let mut sizes = HashMap::new();
    for instance in instances {
        let name = instance.name();
        let Some(body) = instance.body() else {
            report.bodies_unavailable.push(name);
            continue;
        };
        let mut record = |ty: Ty, role: &str, place: String, span: Span| {
            let size = sizes.entry(ty).or_insert_with(|| {
                ty.layout()
                    .map(|layout| layout.shape().size.bytes() as u64)
                    .map_err(|error| format!("{error:?}"))
            });
            match size {
                Ok(bytes) if *bytes > limit => report.records.push(Record {
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
                    if let Ok(ty) = argument.ty(body.locals()) {
                        record(
                            ty,
                            role,
                            format!("bb{block}.arg{index}"),
                            data.terminator.span,
                        );
                    }
                }
            }
        }
    }
    report
        .records
        .sort_by(|a, b| (&a.instance, &a.place).cmp(&(&b.instance, &b.place)));
    report.bodies_unavailable.sort();
    report.errors.sort();
    report.errors.dedup();
    report
}
