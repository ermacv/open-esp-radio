//! The image linker.
//!
//! rustc runs this program in place of `rust-lld` for every image. It reads
//! its own inputs (arguments, `@file` response files, `-l` libraries found on
//! the `-L` paths, and every member of an archive) and refuses a link in
//! which an input section the runtime linker script places in a region the
//! boot zeroes (the chip profile's `[staged] zeroed-inputs`, handed over in
//! `OER_IMAGE_ZEROED_INPUTS`) carries an
//! initializer: bytes other than zero, or a relocation. Those regions are
//! `NOLOAD`, so such an initializer would be dropped without a trace in the
//! image. LLVM emits a named section such as `.psram.bss.x` as `PROGBITS`
//! even for zero bytes, so the type alone does not decide. Otherwise it runs
//! `rust-lld -flavor gnu` with the same arguments, as rustc would for the
//! target's `rust-lld` linker.
//!
//! Runtime images with an interrupt contract then reuse the interrupt
//! analysis to place strict cached code with one additional link.

use std::{
    ffi::{OsStr, OsString},
    fmt, fs,
    path::PathBuf,
};

pub mod strict;
use strict::InputSection;

/// An input section bound for a zeroed region that carries an initializer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedZeroSection {
    /// The input file, and the archive member when it is one.
    pub file: String,
    pub section: String,
    /// Its bytes other than zero.
    pub nonzero_bytes: u64,
    /// Its relocations: addresses the initializer would hold.
    pub relocations: u64,
    /// The symbols defined in the section.
    pub symbols: Vec<String>,
}

impl fmt::Display for InitializedZeroSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: section {} is zeroed at boot but carries an initializer \
             ({} bytes other than zero, {} relocations)",
            self.file, self.section, self.nonzero_bytes, self.relocations
        )?;
        if !self.symbols.is_empty() {
            write!(f, " (symbols: {})", self.symbols.join(", "))?;
        }
        Ok(())
    }
}

/// Run the link: check its inputs, then the real linker. Returns the exit
/// code.
pub fn run(arguments: Vec<OsString>, zeroed: &[String]) -> i32 {
    let mut arguments = match expand_response_files(arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("oer-image-linker: {error}");
            return 1;
        }
    };
    let (violations, sections) = match inspect_inputs(&arguments, zeroed) {
        Ok(inputs) => inputs,
        Err(error) => {
            eprintln!("oer-image-linker: {error}");
            return 1;
        }
    };
    if !violations.is_empty() {
        for violation in &violations {
            eprintln!("oer-image-linker: {violation}");
        }
        eprintln!(
            "oer-image-linker: a static in a zeroed region must start as all zero bytes \
             (`oer_memory::zeroed_static!`)"
        );
        return 1;
    }
    let spec_path = std::env::var_os(strict::SPEC_ENV).map(PathBuf::from);
    match link(&mut arguments, &sections, spec_path.as_deref()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("oer-image-linker: {error}");
            1
        }
    }
}

fn link(
    arguments: &mut Vec<OsString>,
    sections: &[InputSection],
    spec: Option<&std::path::Path>,
) -> crate::Result<i32> {
    let Some(spec_path) = spec else {
        return Ok(run_linker(arguments));
    };
    let mut inputs = oer_os_durable::inputs::Inputs::tracked();
    let spec = strict::Spec::read(spec_path, &mut inputs)?;
    let directory = spec_path
        .parent()
        .ok_or("image linker specification has no directory")?;
    let output = output_file(arguments).ok_or("runtime link has no -o output ELF")?;
    // A previous build's fragment must never affect the first pass.
    std::fs::write(directory.join(strict::FRAGMENT), "")?;
    arguments.push("-L".into());
    arguments.push(directory.into());
    let code = run_linker(arguments);
    if code != 0 {
        return Ok(code);
    }
    let placement = strict::analyze(&spec, &output, sections, &mut inputs)?;
    if !placement.functions.is_empty() {
        std::fs::write(directory.join(strict::FRAGMENT), placement.fragment())?;
        let code = run_linker(arguments);
        if code != 0 {
            return Ok(code);
        }
    }
    strict::save(placement, directory, &output, inputs)?;
    Ok(0)
}

fn output_file(arguments: &[OsString]) -> Option<PathBuf> {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let text = argument.to_str()?;
        if text == "-o" || text == "--output" {
            return arguments.next().map(PathBuf::from);
        }
        if let Some(path) = text
            .strip_prefix("--output=")
            .or_else(|| text.strip_prefix("-o").filter(|path| !path.is_empty()))
        {
            return Some(path.into());
        }
    }
    None
}

/// `rust-lld -flavor gnu` with the arguments, from the `PATH` rustc gives
/// its linker (the sysroot's tool directory).
fn run_linker(arguments: &[OsString]) -> i32 {
    let mut command = oer_os_process::command("rust-lld");
    if arguments.first().map(OsString::as_os_str) != Some(OsStr::new("-flavor")) {
        command.args(["-flavor", "gnu"]);
    }
    match command.args(arguments).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("oer-image-linker: cannot run rust-lld: {error}");
            1
        }
    }
}

/// The arguments with every `@file` replaced by its GNU-tokenized contents,
/// recursively.
pub fn expand_response_files(arguments: Vec<OsString>) -> Result<Vec<OsString>, String> {
    let mut expanded = Vec::new();
    expand_into(arguments, &mut expanded, 0)?;
    Ok(expanded)
}

fn expand_into(
    arguments: Vec<OsString>,
    expanded: &mut Vec<OsString>,
    depth: u32,
) -> Result<(), String> {
    if depth > 16 {
        return Err("response files nest deeper than 16".into());
    }
    for argument in arguments {
        match argument
            .to_str()
            .and_then(|argument| argument.strip_prefix('@'))
        {
            Some(path) => {
                let text = fs::read_to_string(path)
                    .map_err(|error| format!("cannot read response file {path}: {error}"))?;
                expand_into(
                    tokenize_gnu(&text)
                        .into_iter()
                        .map(OsString::from)
                        .collect(),
                    expanded,
                    depth + 1,
                )?;
            }
            None => expanded.push(argument),
        }
    }
    Ok(())
}

/// Split a GNU response file: whitespace separates arguments, a backslash
/// escapes the next character, and single or double quotes group.
pub fn tokenize_gnu(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut started = false;
    let mut quote = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    token.push(next);
                }
                started = true;
            }
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => token.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    tokens.push(core::mem::take(&mut token));
                    started = false;
                }
            }
            (None, c) => {
                token.push(c);
                started = true;
            }
        }
    }
    if started {
        tokens.push(token);
    }
    tokens
}

/// Every input of the link: files named as arguments and `-l` libraries on
/// the `-L` paths, in argument order.
pub fn inputs(arguments: &[OsString]) -> Vec<PathBuf> {
    let mut search = Vec::new();
    let mut libraries = Vec::new();
    let mut files = Vec::new();
    let mut iter = arguments.iter().peekable();
    while let Some(argument) = iter.next() {
        let Some(text) = argument.to_str() else {
            files.push(PathBuf::from(argument));
            continue;
        };
        if text == "-L" || text == "--library-path" {
            if let Some(path) = iter.next() {
                search.push(PathBuf::from(path));
            }
        } else if let Some(path) = text
            .strip_prefix("--library-path=")
            .or(text.strip_prefix("-L"))
        {
            search.push(PathBuf::from(path));
        } else if text == "-l" || text == "--library" {
            if let Some(name) = iter.next().and_then(|name| name.to_str()) {
                libraries.push(name.to_owned());
            }
        } else if let Some(name) = text.strip_prefix("--library=").or(text.strip_prefix("-l")) {
            libraries.push(name.to_owned());
        } else if !text.starts_with('-') {
            files.push(PathBuf::from(text));
        }
    }
    for library in libraries {
        let names = match library.strip_prefix(':') {
            Some(exact) => vec![exact.to_owned()],
            None => vec![format!("lib{library}.a")],
        };
        if let Some(found) = search
            .iter()
            .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
            .find(|path| path.is_file())
        {
            files.push(found);
        }
    }
    files.retain(|path| path.is_file());
    files
}

/// The initialized zero sections of every ELF object among the inputs and
/// their archive members; other inputs (linker scripts, metadata, bitcode)
/// are not objects and carry no sections.
/// The zeroed input patterns the image pipeline handed over.
pub fn zeroed_inputs() -> Vec<String> {
    std::env::var("OER_IMAGE_ZEROED_INPUTS")
        .unwrap_or_default()
        .split(',')
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether an input section named `name` matches one of the `zeroed`
/// patterns: `name` exactly, or `name.*` for any dotted suffix.
pub fn is_zeroed_input(zeroed: &[String], name: &str) -> bool {
    zeroed
        .iter()
        .any(|pattern| match pattern.strip_suffix(".*") {
            Some(prefix) => name
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with('.')),
            None => name == pattern,
        })
}

pub fn check_inputs(
    arguments: &[OsString],
    zeroed: &[String],
) -> Result<Vec<InitializedZeroSection>, String> {
    inspect_inputs(arguments, zeroed).map(|(violations, _)| violations)
}

fn inspect_inputs(
    arguments: &[OsString],
    zeroed: &[String],
) -> Result<(Vec<InitializedZeroSection>, Vec<InputSection>), String> {
    let mut violations = Vec::new();
    let mut sections = Vec::new();
    for input in inputs(arguments) {
        let data = fs::read(&input)
            .map_err(|error| format!("cannot read {}: {error}", input.display()))?;
        check_file(
            &input.display().to_string(),
            &data,
            zeroed,
            &mut violations,
            &mut sections,
        );
    }
    Ok((violations, sections))
}

fn check_file(
    name: &str,
    data: &[u8],
    zeroed: &[String],
    violations: &mut Vec<InitializedZeroSection>,
    sections: &mut Vec<InputSection>,
) {
    if data.starts_with(b"!<arch>\n") {
        // A thin archive holds no members of its own to check.
        for (member, member_data) in oer_analysis_elf::members(data).unwrap_or_default() {
            check_object(
                &format!("{name}({member})"),
                &format!("{name}:{member}"),
                member_data,
                zeroed,
                violations,
                sections,
            );
        }
        return;
    }
    check_object(name, name, data, zeroed, violations, sections);
}

fn check_object(
    name: &str,
    selector: &str,
    data: &[u8],
    zeroed: &[String],
    violations: &mut Vec<InitializedZeroSection>,
    sections: &mut Vec<InputSection>,
) {
    if !data.starts_with(b"\x7fELF") {
        return;
    }
    let Ok(file) = oer_analysis_elf::Elf::parse(data) else {
        return;
    };
    for section in file.sections() {
        if section.executable && section.size > 0 {
            sections.push(InputSection {
                file: selector.to_owned(),
                index: section.index,
                name: section.name.to_owned(),
                size: section.size,
                symbols: file
                    .symbols()
                    .filter(|symbol| symbol.defined && symbol.section == Some(section.index))
                    .map(|symbol| symbol.name.to_owned())
                    .filter(|name| !name.is_empty() && !name.starts_with('$'))
                    .collect(),
            });
        }
        if !is_zeroed_input(zeroed, section.name) || section.nobits {
            continue;
        }
        let nonzero_bytes = section.data.iter().filter(|&&byte| byte != 0).count() as u64;
        let relocations = file
            .relocations(section.index)
            .map_or(0, |relocations| relocations.count() as u64);
        if nonzero_bytes == 0 && relocations == 0 {
            continue;
        }
        let symbols = file
            .symbols()
            .filter(|symbol| symbol.section == Some(section.index))
            .map(|symbol| symbol.name.to_owned())
            .filter(|symbol| !symbol.is_empty() && !symbol.starts_with('$'))
            .collect();
        violations.push(InitializedZeroSection {
            file: name.to_owned(),
            section: section.name.to_owned(),
            nonzero_bytes,
            relocations,
            symbols,
        });
    }
}

#[cfg(test)]
mod tests;
