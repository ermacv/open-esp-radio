//! Command-line adapter for Blobray operations that run in this process.

#![forbid(unsafe_code)]

use clap::{Parser, Subcommand, ValueEnum};
use oer_analysis_blobray_application as app;
use oer_analysis_riscv_model::{
    CallAbi, DEFAULT_WORKING_BYTES, Error, ErrorCode, ReadOnlyWindow, Result,
};
use std::io::Write;
use std::{ffi::OsString, path::PathBuf, process::ExitCode};
mod command_tree;

#[derive(Clone, Copy, Default, ValueEnum)]
enum Format {
    #[default]
    Human,
    Json,
}

/// The calling convention a library analysis may assume.
#[derive(Clone, Copy, ValueEnum)]
enum AbiArg {
    /// The RISC-V integer calling convention: a call keeps sp, gp, tp and
    /// s0-s11.
    RiscvInteger,
}

/// Explicit assumptions of a library analysis; none by default.
#[derive(clap::Args)]
struct Assumptions {
    /// Assume this calling convention at every call the analysis cannot
    /// follow; without it, every register is unknown after such a call and
    /// an access through a preserved register loses its address.
    #[arg(long, value_enum)]
    abi: Option<AbiArg>,
    /// Compute shared result/argument summaries in two analyses, in
    /// callee-first order; result constants propagate through acyclic calls.
    #[arg(long)]
    call_summaries: bool,
    /// START:LENGTH range read-only at run time whatever a linked image's
    /// section flags say, such as flash mapped for reading: constants are
    /// read from it. Take it from the chip's memory map; repeat for more.
    #[arg(long = "read-only", value_name = "START:LENGTH", value_parser = parse_window)]
    read_only: Vec<ReadOnlyWindow>,
}

impl Assumptions {
    fn analysis(&self) -> app::library::Assumptions<'_> {
        app::library::Assumptions {
            abi: self.abi.map(|AbiArg::RiscvInteger| CallAbi::RiscvInteger),
            call_summaries: self.call_summaries,
            read_only: &self.read_only,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "blobray",
    about = "Captured binary research inside this process: library register accesses and function records"
)]
struct Cli {
    #[arg(long, global = true, value_enum, default_value = "human")]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze every function of captured libraries in this process and report
    /// the memory addresses they access, with blocked functions and gaps.
    RegisterAccesses {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// START:LENGTH candidate interval; without one, every numeric address.
        #[arg(long = "range", value_parser = parse_range)]
        ranges: Vec<oer_analysis_blobray_domain::ImageRegion>,
        /// Keep only observations in this 32-bit word; repeat for several.
        /// Blocked functions, gaps and the summary are never filtered.
        #[arg(long = "address", value_parser = parse_number, value_name = "ADDRESS")]
        addresses: Vec<u64>,
        /// Keep only observations of this function; repeat for several.
        #[arg(long = "function", value_name = "NAME")]
        functions: Vec<String>,
        /// Group the selected observations by word or by function. The human
        /// format groups by address when a filter is given without it.
        #[arg(long = "group-by", value_enum)]
        group_by: Option<GroupKey>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// the complete records of the named functions: every fact, expression and
    /// value the analysis derived, with coverage and semantics completeness.
    FunctionRecords {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A function symbol name to report; repeat for several.
        #[arg(long = "function", required = true, value_name = "NAME")]
        functions: Vec<String>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// every memory access that lands on one field offset, as a root symbol,
    /// register or stack and the path of loaded pointers to the field.
    FieldAccesses {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// The field's byte displacement from its pointer or root.
        #[arg(long, allow_negative_numbers = true)]
        offset: i64,
        /// Only accesses of this many bytes.
        #[arg(long)]
        width: Option<u8>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured libraries in this process and report
    /// references through relocations, resolved transfers and stored-address
    /// candidates for the named functions.
    Callers {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A symbol name whose references to report; repeat for several.
        #[arg(long = "symbol", required = true, value_name = "NAME")]
        symbols: Vec<String>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Compare two builds of the same firmware by their MMIO behavior: for
    /// each function, the accesses inside the `--range` windows that one
    /// build has and the other lacks. Symbol hashes, code layout, branch
    /// offsets and the addresses of loaded data do not differ; a stored
    /// image address does only where no sized symbol names it.
    MmioDiff {
        /// The old build: repeat ROLE=PATH or ROLE=pin:CHIP/ID.
        #[arg(long = "old", required = true, value_name = "ROLE=PATH")]
        old: Vec<OsString>,
        /// The new build: repeat ROLE=PATH.
        #[arg(long = "new", required = true, value_name = "ROLE=PATH")]
        new: Vec<OsString>,
        /// START:LENGTH MMIO window; repeat for several.
        #[arg(long = "range", required = true, value_parser = parse_range)]
        ranges: Vec<oer_analysis_blobray_domain::ImageRegion>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// List the defined function and data symbols of captured inputs with
    /// member, section, address, size and their `ROLE[member]::name` citation.
    Symbols {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; use the pinned artifact id as ROLE to print the
        /// citation form provenance checks use.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A substring of the name, or a whole-name pattern with `*`.
        #[arg(long = "match", value_name = "PATTERN")]
        pattern: Option<String>,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// List the NUL-terminated text in the read-only data sections of
    /// captured inputs, with member, section and offset.
    Strings {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a string names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// Only this archive member.
        #[arg(long = "object", value_name = "MEMBER")]
        object: Option<String>,
        /// The shortest string reported.
        #[arg(long = "min-length", default_value_t = app::symbols::MIN_STRING)]
        min_length: usize,
        #[command(flatten)]
        limits: InProcessOptions,
    },
    /// Analyze every function of captured inputs in this process and report
    /// the argument registers a0..a7 at every call site of the named
    /// functions: constants, symbols, the caller's own arguments or unknown.
    CallArguments {
        /// Repeat ROLE=PATH or ROLE=pin:CHIP/ID; a function names its input by position.
        #[arg(long = "input", required = true, value_name = "ROLE=PATH")]
        inputs: Vec<OsString>,
        /// A called function's name; repeat for several.
        #[arg(long = "symbol", required = true, value_name = "NAME")]
        symbols: Vec<String>,
        #[command(flatten)]
        assumptions: Assumptions,
        #[command(flatten)]
        limits: InProcessOptions,
    },
}

/// The `--group-by` key of `register-accesses`.
#[derive(Clone, Copy, ValueEnum)]
enum GroupKey {
    Address,
    Function,
}

impl GroupKey {
    fn by(self) -> app::access_groups::GroupBy {
        match self {
            Self::Address => app::access_groups::GroupBy::Address,
            Self::Function => app::access_groups::GroupBy::Function,
        }
    }
}

const MIB: u64 = 1024 * 1024;

/// Cooperative limits of an operation that runs in this process.
#[derive(clap::Args)]
struct InProcessOptions {
    #[arg(long, default_value_t = DEFAULT_WORKING_BYTES / MIB)]
    working_memory_mib: u64,
    #[arg(long, default_value_t = oer_analysis_riscv_model::DEFAULT_TIMEOUT_MS / 1000)]
    timeout_secs: u64,
    #[arg(long, default_value_t = oer_analysis_riscv_model::DEFAULT_WORK_UNITS)]
    max_work_units: u64,
}

impl InProcessOptions {
    fn memory(&self) -> Result<oer_analysis_riscv_model::WorkingMemory> {
        oer_analysis_riscv_model::WorkingMemory::new(
            self.working_memory_mib
                .checked_mul(MIB)
                .ok_or_else(|| invalid("working memory limit overflow"))?,
        )
    }
    fn control(&self) -> app::in_process::Limits {
        app::in_process::Limits::new(
            self.max_work_units,
            std::time::Duration::from_secs(self.timeout_secs),
        )
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args[1] == command_tree::REQUEST {
        print!("{}", command_tree::json());
        return ExitCode::SUCCESS;
    }
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            if let Some(json) = parse_error_json(&args, &error) {
                eprintln!("{json}");
            } else {
                let _ = error.print();
            }
            return ExitCode::from(error.exit_code() as u8);
        }
    };
    match run(cli.command, cli.format) {
        Ok(status) => status,
        Err(error) => {
            match cli.format {
                Format::Human => eprintln!("blobray: {error}"),
                Format::Json => eprintln!("{}", serde_json::json!({"schema": 1, "error": error})),
            }
            ExitCode::FAILURE
        }
    }
}

/// Only an error's presentation examines argv outside clap. Even a flag
/// after an invalid command or option must select the machine error format.
fn parse_error_json(args: &[OsString], error: &clap::Error) -> Option<String> {
    let json = args.iter().any(|argument| argument == "--format=json")
        || args
            .windows(2)
            .any(|pair| pair[0] == "--format" && pair[1] == "json");
    (error.use_stderr() && json)
        .then(|| serde_json::json!({"schema": 1, "error": invalid(&error.to_string())}).to_string())
}

fn run(command: Command, format: Format) -> Result<ExitCode> {
    match command {
        Command::RegisterAccesses {
            inputs,
            ranges,
            addresses,
            functions,
            group_by,
            assumptions,
            limits,
        } => {
            let words = addresses
                .into_iter()
                .map(|address| {
                    u32::try_from(address)
                        .map(|address| address & !3)
                        .map_err(|_| invalid("--address exceeds 32 bits"))
                })
                .collect::<Result<Vec<_>>>()?;
            let filter = app::access_groups::AccessFilter { words, functions };
            register_accesses(
                inputs,
                ranges,
                filter,
                group_by.map(GroupKey::by),
                assumptions.analysis(),
                limits,
                format,
            )
        }
        Command::FunctionRecords {
            inputs,
            functions,
            assumptions,
            limits,
        } => function_records(inputs, functions, assumptions.analysis(), limits, format),
        Command::FieldAccesses {
            inputs,
            offset,
            width,
            assumptions,
            limits,
        } => field_accesses(
            inputs,
            offset,
            width,
            assumptions.analysis(),
            limits,
            format,
        ),
        Command::Callers {
            inputs,
            symbols,
            assumptions,
            limits,
        } => callers(inputs, symbols, assumptions.analysis(), limits, format),
        Command::CallArguments {
            inputs,
            symbols,
            assumptions,
            limits,
        } => call_arguments(inputs, symbols, assumptions.analysis(), limits, format),
        Command::MmioDiff {
            old,
            new,
            ranges,
            assumptions,
            limits,
        } => mmio_diff(old, new, ranges, assumptions.analysis(), limits, format),
        Command::Symbols {
            inputs,
            pattern,
            limits,
        } => symbols(inputs, pattern, limits, format),
        Command::Strings {
            inputs,
            object,
            min_length,
            limits,
        } => strings(inputs, object, min_length, limits, format),
    }
}

/// Analyze the libraries `inputs` name in this process and print every
/// access of the field at `offset` (of `width` bytes, when given).
fn field_accesses(
    inputs: Vec<OsString>,
    offset: i64,
    width: Option<u8>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut functions = Vec::new();
    let mut blocked = Vec::new();
    let summary = app::library::field_accesses(
        &executables,
        assumptions,
        offset,
        width,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |result, _| {
            match result {
                app::library::FunctionResult::Matched(function) => functions.push(function.clone()),
                app::library::FunctionResult::Blocked { function, .. } => {
                    blocked.push(function.clone())
                }
            }
            Ok(())
        },
    )?;
    let unknown_addresses = summary.unknown_addresses;
    let app::library::QuerySummary {
        partial,
        partial_causes,
        gaps,
        ..
    } = summary.analysis;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::FieldAccessesDocument {
                schema: oer_tool_analysis_cli::wire::FIELD_ACCESSES_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi: assumptions.abi,
                call_returns: assumptions.call_summaries,
                read_only: assumptions.read_only.to_vec(),
                offset,
                width,
                functions,
                blocked,
                partial,
                partial_causes,
                gaps,
                unknown_addresses,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for function in &functions {
                let name = function
                    .function
                    .name
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                for access in &function.accesses {
                    let root = match &access.root {
                        app::field::FieldRoot::Symbol {
                            name: Some(name), ..
                        } => name.clone(),
                        other => format!("{other:?}"),
                    };
                    writeln!(
                        out,
                        "{name} (input {}) +{}: {:?} width {} {root} {:?}",
                        function.function.input,
                        access.offset,
                        access.access,
                        access.width,
                        access.path,
                    )
                    .map_err(io_error)?;
                }
            }
            writeln!(
                out,
                "{} functions access the field; {} functions blocked; {partial} functions analyzed incompletely{}; {gaps} gaps; {unknown_addresses} accesses at unknown addresses",
                functions.len(),
                blocked.len(),
                oer_tool_analysis_cli::partial::human(&partial_causes),
            )
            .map_err(io_error)?;
            if assumptions.abi.is_none() && unknown_addresses > 0 {
                writeln!(
                    out,
                    "without --abi riscv-integer every register is unknown after a call the analysis cannot follow; accesses through a preserved register then have no address"
                )
                .map_err(io_error)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Analyze the libraries `inputs` name in this process and print the complete
/// records of the functions `names` names.
fn function_records(
    inputs: Vec<OsString>,
    names: Vec<String>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    // Image names are presentation, built before the analysis on the same
    // control: one budget for the whole operation, and a completed analysis
    // is never lost to them.
    let images = match format {
        Format::Human => executables
            .iter()
            .map(|executable| {
                app::symbols::image_symbols(executable, &memory, &mut control)
                    .map(|symbols| symbols.map(oer_tool_analysis_cli::listing::ImageSymbols::new))
            })
            .collect::<Result<Vec<_>>>()?,
        Format::Json => Vec::new(),
    };
    // The string sections the named functions' objects hold, read on the
    // same control before the analysis, like the image names.
    let strings = match format {
        Format::Human => {
            app::symbols::string_sections(&executables, &names, &memory, &mut control)?
        }
        Format::Json => std::collections::BTreeMap::new(),
    };
    let wanted = |function: &oer_analysis_blobray_domain::LibraryFunction| {
        function
            .name
            .as_deref()
            .is_some_and(|name| names.iter().any(|wanted| wanted.as_bytes() == name))
    };
    let mut functions = Vec::new();
    app::library::analyze_library(
        &executables,
        assumptions,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |outcome, _| {
            match outcome {
                app::library::LibraryOutcome::Analyzed(analyzed) if wanted(analyzed.function) => {
                    functions.push(oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
                        function: analyzed.function.clone(),
                        complete: analyzed.complete(),
                        coverage: analyzed.coverage,
                        semantics: analyzed.semantics,
                        records: analyzed.records.to_vec(),
                        jump_tables: analyzed.jump_tables.to_vec(),
                    });
                }
                app::library::LibraryOutcome::Blocked { function, error } if wanted(function) => {
                    functions.push(oer_tool_analysis_cli::wire::NamedFunction::Blocked {
                        function: function.clone(),
                        error: error.clone(),
                    });
                }
                _ => {}
            }
            Ok(())
        },
    )?;
    let found = |name: &str| {
        functions.iter().any(|function| {
            let (oer_tool_analysis_cli::wire::NamedFunction::Analyzed { function, .. }
            | oer_tool_analysis_cli::wire::NamedFunction::Blocked { function, .. }) = function;
            function.name.as_deref() == Some(name.as_bytes())
        })
    };
    let missing: Vec<String> = names.iter().filter(|name| !found(name)).cloned().collect();
    let status = if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::FunctionRecordsDocument {
                schema: oer_tool_analysis_cli::wire::FUNCTION_RECORDS_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi: assumptions.abi,
                call_returns: assumptions.call_summaries,
                read_only: assumptions.read_only.to_vec(),
                functions,
                missing,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for function in &functions {
                match function {
                    oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
                        function,
                        complete,
                        records,
                        jump_tables,
                        ..
                    } => writeln!(
                        out,
                        "{} (input {}): {} records, {}",
                        String::from_utf8_lossy(function.name.as_deref().unwrap_or_default()),
                        function.input,
                        records.len(),
                        if *complete { "complete" } else { "incomplete" }
                    )
                    .and_then(|()| {
                        oer_tool_analysis_cli::listing::listing(
                            records,
                            images.get(function.input as usize).and_then(Option::as_ref),
                            &app::symbols::referenced_strings(function, records, &strings),
                        )
                        .iter()
                        .try_for_each(|line| writeln!(out, "{line}"))
                    })
                    .and_then(|()| {
                        jump_tables.iter().try_for_each(|table| {
                            writeln!(out, "{}", oer_tool_analysis_cli::jump_table::human(table))
                        })
                    }),
                    oer_tool_analysis_cli::wire::NamedFunction::Blocked { function, error } => {
                        writeln!(
                            out,
                            "{} (input {}): blocked: {error}",
                            String::from_utf8_lossy(function.name.as_deref().unwrap_or_default()),
                            function.input
                        )
                    }
                }
                .map_err(io_error)?;
            }
            for name in &missing {
                writeln!(out, "{name}: no input defines it").map_err(io_error)?;
            }
        }
    }
    out.flush().map_err(io_error)?;
    Ok(status)
}

/// Compare the MMIO behavior of the builds `old` and `new` names.
fn mmio_diff(
    old: Vec<OsString>,
    new: Vec<OsString>,
    ranges: Vec<oer_analysis_blobray_domain::ImageRegion>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let read = |inputs: Vec<OsString>| -> Result<(Vec<Input>, Vec<app::in_process::Executable>)> {
        let inputs = inputs
            .into_iter()
            .map(parse_input)
            .collect::<Result<Vec<_>>>()?;
        let executables = inputs
            .iter()
            .map(|input| {
                std::fs::read(&input.path)
                    .map(app::in_process::Executable::new)
                    .map_err(io_error)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((inputs, executables))
    };
    let (old_inputs, old) = read(old)?;
    let (new_inputs, new) = read(new)?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let diff = app::mmio_diff::mmio_diff(
        &old,
        &new,
        assumptions,
        &ranges,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
    )?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::MmioDiffDocument {
                schema: oer_tool_analysis_cli::wire::MMIO_DIFF_SCHEMA,
                old: described(&old_inputs, &old),
                new: described(&new_inputs, &new),
                abi: assumptions.abi,
                call_returns: assumptions.call_summaries,
                read_only: assumptions.read_only.to_vec(),
                diff,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            out.write_all(oer_tool_analysis_cli::mmio::render(&diff).as_bytes())
                .map_err(io_error)?;
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// List the symbols of `inputs`, optionally those `pattern` selects.
fn symbols(
    inputs: Vec<OsString>,
    pattern: Option<String>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut rows = Vec::new();
    for (index, (input, executable)) in inputs.iter().zip(&executables).enumerate() {
        let inventory = app::captured::inventory(executable, &memory, &mut control)?;
        rows.extend(app::symbols::rows(
            index as u64,
            &input.role,
            &inventory,
            pattern.as_deref(),
        ));
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::SymbolsDocument {
                schema: oer_tool_analysis_cli::wire::SYMBOLS_SCHEMA,
                inputs: described(&inputs, &executables),
                symbols: rows,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for row in &rows {
                writeln!(
                    out,
                    "{:#010x} {:>6x} {:<8} {:<6} {:<24} {}",
                    row.address,
                    row.size,
                    row.kind,
                    row.binding,
                    row.section.as_deref().unwrap_or("-"),
                    row.citation
                )
                .map_err(io_error)?;
            }
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// List the strings of the read-only data sections of `inputs`.
fn strings(
    inputs: Vec<OsString>,
    member: Option<String>,
    minimum: usize,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let rows = app::symbols::string_rows(
        &executables,
        member.as_deref(),
        minimum,
        &memory,
        &mut control,
    )?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::StringsDocument {
                schema: oer_tool_analysis_cli::wire::STRINGS_SCHEMA,
                inputs: described(&inputs, &executables),
                strings: rows,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for row in &rows {
                writeln!(
                    out,
                    "{}[{}] {}+{:#x}: \"{}\"",
                    inputs[row.input as usize].role,
                    row.member.as_deref().unwrap_or(""),
                    row.section.as_deref().unwrap_or("-"),
                    row.offset,
                    row.text
                )
                .map_err(io_error)?;
            }
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Each input's role and content identity.
fn described(
    inputs: &[Input],
    executables: &[app::in_process::Executable],
) -> Vec<oer_tool_analysis_cli::wire::RegisterAccessInput> {
    inputs
        .iter()
        .zip(executables)
        .map(
            |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                role: input.role.clone(),
                sha256: executable.id().clone(),
            },
        )
        .collect()
}

fn io_error(e: std::io::Error) -> Error {
    oer_analysis_riscv_model::storage_io(e)
}

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}

/// One `ROLE=PATH` or `ROLE=pin:CHIP/ID` input binding.
struct Input {
    role: String,
    path: PathBuf,
}

/// The repository this tool runs in, whose pins name the vendor artifacts.
fn root() -> Result<PathBuf> {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(io_error)
}

/// `ROLE=PATH`, or `ROLE=pin:CHIP/ID` for the pinned vendor artifact `ID`
/// of `CHIP`, fetched and verified against its SHA-256. A vendor artifact
/// is read only by its pin: a path into the vendor store is refused.
fn parse_input(value: OsString) -> Result<Input> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = value.as_os_str().as_bytes();
    let split = bytes
        .iter()
        .position(|b| *b == b'=')
        .ok_or_else(|| invalid("expected ROLE=PATH or ROLE=pin:CHIP/ID"))?;
    let role = std::str::from_utf8(&bytes[..split])
        .map_err(|_| invalid("role must be UTF-8"))?
        .to_owned();
    if split + 1 == bytes.len() {
        return Err(invalid("input path is empty"));
    }
    let value = &bytes[split + 1..];
    if let Some(pin) = value.strip_prefix(b"pin:") {
        let pin = std::str::from_utf8(pin).map_err(|_| invalid("pin must be UTF-8"))?;
        let (chip, id) = pin
            .split_once('/')
            .ok_or_else(|| invalid("expected pin:CHIP/ID"))?;
        let path = oer_tool_pins::artifacts::ensure(&root()?, chip, id)
            .map_err(|error| invalid(&format!("pin {pin}: {error}")))?;
        return Ok(Input { role, path });
    }
    let path = PathBuf::from(OsString::from_vec(value.to_vec()));
    refuse_store_path(&path)?;
    Ok(Input { role, path })
}

/// Refuse `path` when it lies in the vendor store: a vendor artifact is
/// read by its pin, which verifies it.
fn refuse_store_path(path: &std::path::Path) -> Result<()> {
    let store = oer_tool_pins::artifacts::store()
        .map_err(|error| invalid(&format!("vendor store: {error}")))?;
    refuse_inside(path, &[&store, &root()?.join("target/vendor")])
}

/// Refuse `path` when it resolves below one of `stores`.
fn refuse_inside(path: &std::path::Path, stores: &[&std::path::Path]) -> Result<()> {
    let Ok(resolved) = path.canonicalize() else {
        return Ok(());
    };
    if stores.iter().any(|store| {
        store
            .canonicalize()
            .is_ok_and(|store| resolved.starts_with(store))
    }) {
        return Err(invalid(&format!(
            "{} is a vendor artifact: name it by its pin, ROLE=pin:CHIP/ID",
            path.display()
        )));
    }
    Ok(())
}

fn parse_number(v: &str) -> std::result::Result<u64, String> {
    if let Some(hex) = v.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        v.parse::<u64>()
    }
    .map_err(|e| e.to_string())
}

fn parse_range(
    text: &str,
) -> std::result::Result<oer_analysis_blobray_domain::ImageRegion, String> {
    let (start, length) = text.split_once(':').ok_or("expected START:LENGTH")?;
    Ok(oer_analysis_blobray_domain::ImageRegion {
        start: u32::try_from(parse_number(start)?).map_err(|e| e.to_string())?,
        length: parse_number(length)?,
    })
}

fn parse_window(text: &str) -> std::result::Result<ReadOnlyWindow, String> {
    let range = parse_range(text)?;
    range
        .end()
        .ok_or("the window exceeds the RV32 address space")?;
    Ok(ReadOnlyWindow {
        start: range.start,
        length: range.length,
    })
}

fn json_error(e: serde_json::Error) -> Error {
    invalid(&e.to_string())
}

fn register_accesses(
    inputs: Vec<OsString>,
    ranges: Vec<oer_analysis_blobray_domain::ImageRegion>,
    filter: app::access_groups::AccessFilter,
    group_by: Option<app::access_groups::GroupBy>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let json = matches!(format, Format::Json);
    let filtered = !filter.words.is_empty() || !filter.functions.is_empty();
    let group_by = match (group_by, json, filtered) {
        (None, false, true) => Some(app::access_groups::GroupBy::Address),
        (group_by, _, _) => group_by,
    };
    let mut groups = group_by.map(app::access_groups::AccessGroups::new);
    if json {
        let described: Vec<_> = inputs
            .iter()
            .zip(&executables)
            .map(
                |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                    role: input.role.clone(),
                    sha256: executable.id().clone(),
                },
            )
            .collect();
        write!(
            out,
            "{{\"schema\":{},\"inputs\":{},\"abi\":{},\"call_returns\":{},\"read_only\":{},\"records\":[",
            oer_tool_analysis_cli::wire::REGISTER_ACCESSES_SCHEMA,
            serde_json::to_string(&described).map_err(json_error)?,
            serde_json::to_string(&assumptions.abi).map_err(json_error)?,
            assumptions.call_summaries,
            serde_json::to_string(assumptions.read_only).map_err(json_error)?
        )
        .map_err(io_error)?;
    }
    let mut first = true;
    let summary = app::library::register_accesses(
        &executables,
        assumptions,
        &ranges,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            if !filter.keeps(record) {
                return Ok(());
            }
            if let Some(groups) = groups.as_mut() {
                groups.add(record);
            }
            if json {
                if !first {
                    out.write_all(b",").map_err(io_error)?;
                }
                first = false;
                serde_json::to_writer(&mut out, record).map_err(json_error)?;
            }
            Ok(())
        },
    )?;
    if json {
        out.write_all(b"]").map_err(io_error)?;
        if let Some(groups) = &groups {
            write!(
                out,
                ",\"groups\":{}",
                serde_json::to_string(&groups.groups()).map_err(json_error)?
            )
            .map_err(io_error)?;
        }
        write!(
            out,
            ",\"summary\":{}}}",
            serde_json::to_string(&summary).map_err(json_error)?
        )
        .map_err(io_error)?;
        writeln!(out).map_err(io_error)?;
    } else {
        if let Some(groups) = &groups {
            for line in oer_tool_analysis_cli::access_groups::human(&groups) {
                writeln!(out, "{line}").map_err(io_error)?;
            }
        }
        writeln!(
            out,
            "{} functions ({} partial{}, {} blocked), {} gaps, {} observations ({} unresolved, {} indexed)",
            summary.functions,
            summary.partial_functions,
            oer_tool_analysis_cli::partial::human(&summary.partial_causes),
            summary.blocked_functions,
            summary.gaps,
            summary.observations,
            summary.unresolved_addresses,
            summary.indexed_addresses
        )
        .map_err(io_error)?;
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Analyze the inputs in this process and print the argument registers at
/// every call site of a function `symbols` names.
fn call_arguments(
    inputs: Vec<OsString>,
    symbols: Vec<String>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut callers = Vec::new();
    let mut blocked = Vec::new();
    let app::library::QuerySummary {
        partial,
        partial_causes,
        gaps,
        ..
    } = app::library::call_arguments(
        &executables,
        assumptions,
        &symbols,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |result, _| {
            match result {
                app::library::FunctionResult::Matched(function) => callers.push(function.clone()),
                app::library::FunctionResult::Blocked { function, .. } => {
                    blocked.push(function.clone())
                }
            }
            Ok(())
        },
    )?;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::CallArgumentsDocument {
                schema: oer_tool_analysis_cli::wire::CALL_ARGUMENTS_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi: assumptions.abi,
                call_returns: assumptions.call_summaries,
                read_only: assumptions.read_only.to_vec(),
                symbols,
                callers,
                blocked,
                partial,
                partial_causes,
                gaps,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            let mut count = 0;
            for caller in &callers {
                let name = caller
                    .function
                    .name
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                for site in &caller.sites {
                    count += 1;
                    let arguments = if site.arguments.is_empty() {
                        String::from("no register state")
                    } else {
                        site.arguments
                            .iter()
                            .enumerate()
                            .map(|(index, value)| {
                                format!(
                                    "a{index}={}",
                                    oer_tool_analysis_cli::call_arguments::human(value)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    writeln!(
                        out,
                        "{name} (input {}) +{:x}: {} {}({arguments}){}{}",
                        caller.function.input,
                        site.offset,
                        oer_tool_analysis_cli::references::kind_human(site.kind),
                        String::from_utf8_lossy(&site.target),
                        destination_human(site.destination.as_ref()),
                        alternatives_human(site.alternatives),
                    )
                    .map_err(io_error)?;
                }
            }
            writeln!(
                out,
                "{count} call sites in {} functions; {partial} partial{}, {} blocked, {gaps} gaps",
                callers.len(),
                oer_tool_analysis_cli::partial::human(&partial_causes),
                blocked.len(),
            )
            .map_err(io_error)?;
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

/// Analyze the libraries `inputs` name in this process and print every
/// reference their functions make to a symbol `symbols` names.
fn callers(
    inputs: Vec<OsString>,
    symbols: Vec<String>,
    assumptions: app::library::Assumptions<'_>,
    limits: InProcessOptions,
    format: Format,
) -> Result<ExitCode> {
    let inputs = inputs
        .into_iter()
        .map(parse_input)
        .collect::<Result<Vec<_>>>()?;
    let executables = inputs
        .iter()
        .map(|input| {
            std::fs::read(&input.path)
                .map(app::in_process::Executable::new)
                .map_err(io_error)
        })
        .collect::<Result<Vec<_>>>()?;
    let memory = limits.memory()?;
    let mut control = limits.control();
    let mut callers = Vec::new();
    let mut blocked = Vec::new();
    let mut stored = Vec::new();
    let summary = app::library::callers(
        &executables,
        assumptions,
        &symbols,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |result, _| {
            match result {
                app::library::CallerResult::Function(app::library::FunctionResult::Matched(
                    function,
                )) => callers.push(function.clone()),
                app::library::CallerResult::Function(app::library::FunctionResult::Blocked {
                    function,
                    ..
                }) => blocked.push(function.clone()),
                app::library::CallerResult::Stored(reference) => stored.push(reference.clone()),
            }
            Ok(())
        },
    )?;
    let app::library::QuerySummary {
        functions,
        partial,
        partial_causes,
        gaps,
        ..
    } = summary.analysis;
    let scan = summary.scan;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    match format {
        Format::Json => {
            let document = oer_tool_analysis_cli::wire::CallersDocument {
                schema: oer_tool_analysis_cli::wire::CALLERS_SCHEMA,
                inputs: inputs
                    .iter()
                    .zip(&executables)
                    .map(
                        |(input, executable)| oer_tool_analysis_cli::wire::RegisterAccessInput {
                            role: input.role.clone(),
                            sha256: executable.id().clone(),
                        },
                    )
                    .collect(),
                abi: assumptions.abi,
                call_returns: assumptions.call_summaries,
                read_only: assumptions.read_only.to_vec(),
                symbols,
                callers,
                stored,
                scan,
                functions,
                partial,
                partial_causes,
                blocked,
                gaps,
            };
            serde_json::to_writer(&mut out, &document).map_err(json_error)?;
            writeln!(out).map_err(io_error)?;
        }
        Format::Human => {
            for caller in &callers {
                let name = caller
                    .function
                    .name
                    .as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                for reference in &caller.references {
                    writeln!(
                        out,
                        "{name} (input {}) +{:x}: {}{} {}{}{}",
                        caller.function.input,
                        reference.offset,
                        oer_tool_analysis_cli::references::kind_human(reference.kind),
                        reference
                            .relocation_kind
                            .map(|kind| format!(" {kind:?}"))
                            .unwrap_or_default(),
                        String::from_utf8_lossy(&reference.target),
                        destination_human(reference.destination.as_ref()),
                        alternatives_human(reference.alternatives),
                    )
                    .map_err(io_error)?;
                }
            }
            writeln!(
                out,
                "stored: candidates for indirect calls, not proven calls"
            )
            .map_err(io_error)?;
            for reference in &stored {
                use app::references::Stored;
                match reference {
                    Stored::StoredWord { input, section, section_name, address, owners, destination } => {
                        let owners = owners.iter().map(|owner| String::from_utf8_lossy(owner.name.as_deref().unwrap_or_default())).collect::<Vec<_>>().join(", ");
                        writeln!(out, "stored-word (input {input}) section {section} {} {address:#x} owner [{}]: {}{}",
                            String::from_utf8_lossy(section_name.as_deref().unwrap_or_default()), owners,
                            String::from_utf8_lossy(&destination.name), destination_human(Some(destination)))
                    }
                    Stored::Materialized { function, offset, register, destination } => {
                        writeln!(out, "materialized {} (input {}) +{offset:x} x{register}: {}{}",
                            String::from_utf8_lossy(function.name.as_deref().unwrap_or_default()), function.input,
                            String::from_utf8_lossy(&destination.name), destination_human(Some(destination)))
                    }
                }.map_err(io_error)?;
            }
            writeln!(out, "scanned {} sections, {} words; {} NOBITS sections without captured words; {} incomplete inventories", scan.sections, scan.words, scan.nobits_sections, scan.incomplete_inventories).map_err(io_error)?;
            writeln!(
                out,
                "{} functions reference the symbols; {functions} analyzed, {partial} partial{}, {} blocked; {gaps} gaps",
                callers.len(),
                oer_tool_analysis_cli::partial::human(&partial_causes),
                blocked.len()
            )
            .map_err(io_error)?;
        }
    }
    out.flush().map_err(io_error)?;
    Ok(ExitCode::SUCCESS)
}

fn destination_human(target: Option<&app::references::Target>) -> String {
    target
        .map(|target| format!(" (target input {}, {:#x})", target.input, target.address))
        .unwrap_or_default()
}

fn alternatives_human(count: Option<usize>) -> String {
    count
        .map(|count| format!(" (one of {count} targets)"))
        .unwrap_or_default()
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn a_path_into_a_vendor_store_is_refused_and_others_pass() {
        let store = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let pinned = store.path().join("libphy.a");
        let own = other.path().join("image.elf");
        std::fs::write(&pinned, b"x").unwrap();
        std::fs::write(&own, b"x").unwrap();
        let error = refuse_inside(&pinned, &[store.path()]).unwrap_err();
        assert!(error.to_string().contains("ROLE=pin:CHIP/ID"), "{error}");
        refuse_inside(&own, &[store.path()]).unwrap();
    }

    #[test]
    fn a_pin_names_its_chip_and_id() {
        let error = parse_input("lib=pin:libphy".into()).err().unwrap();
        assert!(error.to_string().contains("pin:CHIP/ID"), "{error}");
        let error = parse_input("lib=pin:esp32s31/no-such-artifact".into())
            .err()
            .unwrap();
        assert!(error.to_string().contains("no-such-artifact"), "{error}");
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn grammar_and_successful_format_are_owned_by_clap() {
        Cli::command().debug_assert();
        let cli = Cli::try_parse_from([
            "blobray",
            "--format=json",
            "register-accesses",
            "--input",
            "library=a.elf",
        ])
        .unwrap();
        assert!(matches!(cli.format, Format::Json));
    }

    #[test]
    fn parse_errors_keep_json_even_past_the_first_invalid_argument() {
        for args in [
            vec!["blobray", "unknown", "--format=json"],
            vec!["blobray", "--format=json", "unknown"],
            vec!["blobray", "register-accesses", "--bad", "--format=json"],
            vec!["blobray", "register-accesses", "--format=json", "--bad"],
            vec!["blobray", "unknown", "--format", "json"],
        ] {
            let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
            let error = Cli::try_parse_from(&args)
                .err()
                .expect("invalid invocation");
            let json = parse_error_json(&args, &error).expect("JSON error");
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed["schema"], 1);
            assert_eq!(
                parsed["error"],
                serde_json::to_value(invalid(&error.to_string())).unwrap()
            );
            assert_eq!(error.exit_code(), 2);
        }
        let error = Cli::try_parse_from(["blobray", "unknown"]).err().unwrap();
        assert!(parse_error_json(&["blobray".into(), "unknown".into()], &error).is_none());
    }
}
