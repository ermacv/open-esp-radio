//! Provenance of recovered vendor facts: the code fingerprint of every vendor
//! function production and the register model cite.
//!
//! Production records recovered facts in `SOURCE:` comment blocks, and the
//! register model in its evidence source and register/field descriptions. Every
//! identifier in those texts that names a function of a pinned vendor
//! artifact is a reference. The chip's registry records, for each referenced
//! function, the code fingerprint of the revision its facts were reviewed
//! against. `cargo verification check provenance` fails when a referenced function
//! changed, disappeared or is not registered, so a pin update cannot leave a
//! recovered fact silently describing older code. After reviewing a changed
//! function, `cargo verification provenance --accept NAME` records its pinned
//! fingerprint; the registry diff is the review record. `NAME` is a symbol,
//! or the `artifact[member]::symbol` form the check prints, which narrows it
//! to that member; a name neither registered nor pinned is an error.
//!
//! A citation in the `artifact[member]::symbol` form
//! (`libpp[pm.o]::pm_parse_beacon`, `rom[]::pm_tbtt_process`) cites only that
//! copy, so a fact reviewed against one copy does not claim the others;
//! naming a copy no pinned artifact defines is an error. A bare name cites
//! the function's one pinned copy; a function several pinned artifacts
//! define (the ROM's copy and a library's, which may handle the same
//! register bits differently) must be cited by its copy, and a bare name of
//! one is an error. The platform's ROM function summaries cite the ROM's
//! copy. A coverage decision names the function a scenario executed,
//! whichever copy its link took, and cites every copy until coverage
//! locations carry the copy (). Accepting a copy no citation names any longer removes its
//! registration.
use crate::Result;
use crate::fingerprint::Function;
use oer_vendor_artifacts::project::{self, Project};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Tracked registry of `chip`, relative to the repository root.
fn registry_path(root: &Path, chip: &str) -> Result<String> {
    Ok(Project::new(root, chip)?.provenance_registry())
}

/// Production sources scanned for `SOURCE:` blocks: Rust files and, with
/// `#` comments, TOML files such as the platform's ROM function summaries.
const PRODUCTION: [&str; 2] = ["crates", "platform"];
/// The chip's reviewed verification decisions, relative to its
/// verification directory: the coverage decisions' data and the scenario
/// crate's decision modules.
const DECISIONS: [&str; 2] = ["decisions", "scenarios/src/decisions"];
/// Extensions of the decision files.
const DECISION_EXTENSIONS: [&str; 2] = ["toml", "rs"];
/// Shortest identifier taken as a reference; shorter words are prose.
const MINIMUM_NAME: usize = 4;

/// One registered function definition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub artifact: String,
    pub member: String,
    pub symbol: String,
    pub code: String,
    /// The verification decision files that name the function: their
    /// exclusions describe its code at `code`.
    pub decisions: Vec<String>,
}

fn parse_registry(text: &str) -> Result<Vec<Entry>> {
    let table: toml::Table = toml::from_str(text)?;
    if table.get("schema").and_then(|v| v.as_integer()) != Some(1) {
        return Err("unsupported provenance registry schema".into());
    }
    let mut entries = vec![];
    for value in table
        .get("function")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let field = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .ok_or_else(|| format!("registry function lacks `{key}`").into())
        };
        let decisions = value
            .get("decisions")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "registry decision is not a file name".into())
            })
            .collect::<Result<Vec<String>>>()?;
        entries.push(Entry {
            artifact: field("artifact")?,
            member: field("member")?,
            symbol: field("symbol")?,
            code: field("code")?,
            decisions,
        });
    }
    Ok(entries)
}

fn render_registry(entries: &[Entry]) -> String {
    let mut text = String::from(
        "# Code fingerprints of the vendor functions production `SOURCE:` blocks\n\
         # and the register model cite, and those the verification decisions\n\
         # exclude code of, as reviewed. Maintained by\n\
         # `cargo verification provenance`; checked by `cargo verification\n\
         # check provenance`.\n\
         schema = 1\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "\n[[function]]\nartifact = \"{}\"\nmember = \"{}\"\nsymbol = \"{}\"\ncode = \"{}\"\n",
            entry.artifact, entry.member, entry.symbol, entry.code
        ));
        if !entry.decisions.is_empty() {
            let files: Vec<String> = entry.decisions.iter().map(|f| format!("\"{f}\"")).collect();
            text.push_str(&format!("decisions = [{}]\n", files.join(", ")));
        }
    }
    text
}

/// Code-shaped identifiers of `text`. A word directly followed by `.o` is
/// an archive member name such as `phy_init.o`, not a function.
fn identifiers(text: &str, out: &mut BTreeSet<String>) {
    let mut word = String::new();
    let characters: Vec<char> = text.chars().chain([' ']).collect();
    for (index, &c) in characters.iter().enumerate() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            let member = c == '.'
                && characters.get(index + 1) == Some(&'o')
                && !characters
                    .get(index + 2)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
            if word.len() >= MINIMUM_NAME
                && !member
                && !word.starts_with(|c: char| c.is_ascii_digit())
                && is_code_name(&word)
            {
                out.insert(std::mem::take(&mut word));
            }
            word.clear();
        }
    }
}

/// The `artifact[member]::symbol` forms of `text`, and the text without
/// them. Only a form whose `artifact` is a pinned artifact id is a
/// qualified citation; the survey cites any other's symbol bare.
fn qualified_citations(text: &str) -> (String, Vec<(String, String, String)>) {
    let identifier = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut bare = String::with_capacity(text.len());
    let mut copies = vec![];
    let mut rest = text;
    while let Some(at) = rest.find("]::") {
        let before = &rest[..at];
        let parsed = before.rfind('[').and_then(|open| {
            let member = &before[open + 1..];
            let artifact_start = before[..open]
                .rfind(|c: char| !(identifier(c) || c == '-' || c == '.'))
                .map_or(0, |i| i + 1);
            let artifact = &before[artifact_start..open];
            let after = &rest[at + 3..];
            let symbol_len = after.find(|c: char| !identifier(c)).unwrap_or(after.len());
            let symbol = &after[..symbol_len];
            (!artifact.is_empty() && !symbol.is_empty() && !member.contains(char::is_whitespace))
                .then(|| {
                    (
                        artifact_start,
                        artifact,
                        member,
                        symbol,
                        at + 3 + symbol_len,
                    )
                })
        });
        match parsed {
            Some((start, artifact, member, symbol, end)) => {
                bare.push_str(&rest[..start]);
                bare.push(' ');
                copies.push((artifact.to_owned(), member.to_owned(), symbol.to_owned()));
                rest = &rest[end..];
            }
            None => {
                bare.push_str(&rest[..at + 3]);
                rest = &rest[at + 3..];
            }
        }
    }
    bare.push_str(rest);
    (bare, copies)
}

/// Whether `word` has the shape of a code identifier rather than prose: an
/// underscore, a digit, or an uppercase letter after the first. A plain
/// lowercase word such as `main` or `abort` is prose even when a vendor
/// symbol shares its name.
fn is_code_name(word: &str) -> bool {
    word.contains('_')
        || word.chars().any(|c| c.is_ascii_digit())
        || word.chars().skip(1).any(|c| c.is_ascii_uppercase())
}

/// Code-shaped string literals of every decision file of `directory`, by
/// name, with the files that name them: a decision's `Place` names the
/// vendor function whose code it excludes.
fn decision_words(directory: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut words: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(words);
    };
    for entry in entries {
        let path = entry?.path();
        if !path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DECISION_EXTENSIONS.contains(&e))
        {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("decision file name")?
            .to_owned();
        for literal in std::fs::read_to_string(&path)?
            .split('"')
            .skip(1)
            .step_by(2)
        {
            if !literal.is_empty()
                && literal
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && is_code_name(literal)
            {
                words
                    .entry(literal.to_owned())
                    .or_default()
                    .insert(file.clone());
            }
        }
    }
    Ok(words)
}

/// The chips with a vendor verification project, which a citation may name.
fn citable_chips(root: &Path, supported: &[String]) -> Vec<String> {
    supported
        .iter()
        .filter(|chip| Project::new(root, chip).is_ok())
        .cloned()
        .collect()
}

/// Identifiers of every `SOURCE:` comment block under `directory` that cites
/// `chip`, a violation for every block that is malformed or names its chips
/// wrongly, and the location and identifiers of every chip-neutral block
/// that names no chip (see `oer_markers::source`). Paths are reported
/// and placed relative to `base`.
/// A violation for every chip-neutral block without a chip list that cites
/// one of the scanned chip's `known` functions. A block that cites no vendor
/// function, such as a standard or a HIL record, needs no chip.
fn uncharted_citations(
    uncharted: &[(String, BTreeSet<String>)],
    known: &BTreeSet<&str>,
) -> Vec<String> {
    uncharted
        .iter()
        .filter_map(|(location, block)| {
            block
                .iter()
                .find(|w| known.contains(w.as_str()))
                .map(|function| {
                    format!(
                        "{location}: SOURCE block cites `{function}` under a chip-neutral path but \
                     names no chip"
                    )
                })
        })
        .collect()
}

/// The chip a production scan attributes blocks to.
struct Scan<'a> {
    chip: &'a str,
    supported: &'a [String],
    /// Chips a citation may name.
    citable: &'a [String],
}

/// What a production scan found.
#[derive(Default)]
struct Found {
    words: BTreeSet<String>,
    /// Copies cited by `artifact[member]::symbol`, by symbol.
    qualified: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Where each qualified citation was written, for its diagnostics.
    qualified_at: Vec<(String, String, String, String)>,
    problems: Vec<String>,
    /// Location and identifiers of every neutral block that names no chip.
    uncharted: Vec<(String, BTreeSet<String>)>,
    /// Where each bare citation was written, with the identifiers it names.
    bare_at: Vec<(String, BTreeSet<String>)>,
}

impl Found {
    /// Record the citations of `text`, written at `location`: its
    /// `artifact[member]::symbol` copies and its bare identifiers.
    fn cite(&mut self, location: &str, text: &str) {
        let (bare, copies) = qualified_citations(text);
        let mut words = BTreeSet::new();
        identifiers(&bare, &mut words);
        self.words.extend(words.iter().cloned());
        self.bare_at.push((location.to_owned(), words));
        for (artifact, member, symbol) in copies {
            self.qualified_at.push((
                location.to_owned(),
                artifact.clone(),
                member.clone(),
                symbol.clone(),
            ));
            self.qualified
                .entry(symbol)
                .or_default()
                .insert((artifact, member));
        }
    }
}

/// The production files below `directory` (through the repository model's
/// file inventory; every file for `""`) and their `SOURCE` blocks.
fn production_words(
    scan: &Scan<'_>,
    repo: &crate::files::Files,
    directory: &str,
    found: &mut Found,
) -> Result<()> {
    use oer_markers::source::{Attribution, Syntax, attribute, blocks_in, place};
    let files: Box<dyn Iterator<Item = &str>> = if directory.is_empty() {
        Box::new(repo.files())
    } else {
        Box::new(repo.below(directory))
    };
    for file in files {
        let name = file.rsplit('/').next().unwrap_or(file);
        if let Some(syntax) = Syntax::of(name) {
            let relative = Path::new(file);
            let Some(place) = place(relative, scan.chip, scan.supported) else {
                continue;
            };
            let text = repo.read(file)?;
            let blocks = match blocks_in(&text, syntax) {
                Ok(blocks) => blocks,
                Err(malformed) => {
                    found.problems.push(format!(
                        "{}:{}: malformed SOURCE marker: {}",
                        relative.display(),
                        malformed.line,
                        malformed.reason
                    ));
                    continue;
                }
            };
            for block in blocks {
                match attribute(&block, place, scan.chip, scan.citable) {
                    Attribution::Cited => found.cite(
                        &format!("{}:{}", relative.display(), block.line),
                        &block.text,
                    ),
                    Attribution::NotCited => {}
                    Attribution::Uncharted => {
                        let mut words = BTreeSet::new();
                        identifiers(&block.text, &mut words);
                        found
                            .uncharted
                            .push((format!("{}:{}", relative.display(), block.line), words));
                    }
                    Attribution::Invalid(reason) => found.problems.push(format!(
                        "{}:{}: SOURCE block {reason}",
                        relative.display(),
                        block.line
                    )),
                }
            }
        }
    }
    Ok(())
}

/// The function names of the platform's ROM summaries at `path`: each one
/// cites its function whatever the name's shape (`memset` is code there, not
/// prose). No file, no names.
fn summary_names(path: &Path) -> Result<BTreeSet<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(BTreeSet::new());
    };
    let table: toml::Table =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut names = BTreeSet::new();
    for function in table
        .get("function")
        .and_then(|f| f.as_array())
        .into_iter()
        .flatten()
    {
        let name = function
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{}: a [[function]] without a name", path.display()))?;
        names.insert(name.to_owned());
    }
    Ok(names)
}

/// Every function of every fetched vendor artifact of `chip`, by artifact,
/// and the name of every symbol they define.
#[allow(clippy::type_complexity, reason = "read once and split by the survey")]
fn pinned_functions(
    root: &Path,
    chip: &str,
) -> Result<(BTreeMap<String, (PathBuf, Vec<Function>)>, BTreeSet<String>)> {
    let mut out = BTreeMap::new();
    let mut symbols = BTreeSet::new();
    for artifact in oer_vendor_artifacts::pinned(root, chip)? {
        if artifact.firmware {
            continue;
        }
        let bytes = std::fs::read(&artifact.path)
            .map_err(|e| format!("{}: {e}", artifact.path.display()))?;
        if !oer_elf::is_binary(&bytes) {
            continue;
        }
        let (functions, defined) = crate::fingerprint::functions_and_symbols(&bytes)?;
        symbols.extend(defined);
        out.insert(artifact.id, (artifact.path, functions));
    }
    Ok((out, symbols))
}

struct Survey {
    /// Malformed or wrongly attributed `SOURCE` blocks.
    citations: Vec<String>,
    registry: Vec<Entry>,
    /// Current definitions: (artifact, member, symbol) -> code.
    current: BTreeMap<(String, String, String), String>,
    /// Referenced names: bare cited identifiers that name a current or
    /// registered function, citing every copy.
    references: BTreeSet<String>,
    /// Copies cited by `artifact[member]::symbol`, by symbol.
    qualified: BTreeMap<String, BTreeSet<(String, String)>>,
    /// Each qualified citation with where it was written.
    qualified_at: Vec<(String, String, String, String)>,
    /// Where each bare citation was written, with the identifiers it names.
    bare_at: Vec<(String, BTreeSet<String>)>,
    /// Every cited identifier, whether or not it names a function.
    words: BTreeSet<String>,
    /// Names the verification decisions cite, with the files citing them.
    decisions: BTreeMap<String, BTreeSet<String>>,
    pinned: BTreeMap<String, (PathBuf, Vec<Function>)>,
    /// Obfuscated vendor symbols the chip's vendor documents name that no
    /// pinned artifact defines.
    documents: Vec<String>,
}

fn survey(root: &Path, chip: &str) -> Result<Survey> {
    let registry = match std::fs::read_to_string(root.join(registry_path(root, chip)?)) {
        Ok(text) => parse_registry(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => return Err(error.into()),
    };
    let (pinned, defined) = pinned_functions(root, chip)?;
    let mut current = BTreeMap::new();
    for (artifact, (_, functions)) in &pinned {
        for f in functions {
            current.insert(
                (artifact.clone(), f.member.clone(), f.name.clone()),
                f.code.clone(),
            );
        }
    }
    let supported = project::supported(root)?;
    let scanned = Project::new(root, chip)?;
    let citable = citable_chips(root, &supported);
    let mut found = Found::default();
    let repo = crate::files::Files::load(root)?;
    for directory in PRODUCTION {
        production_words(
            &Scan {
                chip: scanned.name(),
                supported: &supported,
                citable: &citable,
            },
            &repo,
            directory,
            &mut found,
        )?;
    }
    for (file, text) in oer_register_model::descriptions(root, chip)? {
        found.cite(&file.display().to_string(), &text);
    }
    let Found {
        mut words,
        qualified: forms,
        qualified_at: forms_at,
        problems: mut citations,
        uncharted,
        mut bare_at,
    } = found;
    // `libpp[pm.o]::f` names one copy; `libpp.a[pm.o]::f` or any other
    // form whose artifact is no pinned id cites `f` bare.
    let mut qualified: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for (symbol, copies) in forms {
        for (artifact, member) in copies {
            if pinned.contains_key(&artifact) {
                qualified
                    .entry(symbol.clone())
                    .or_default()
                    .insert((artifact, member));
            } else {
                words.insert(symbol.clone());
            }
        }
    }
    let qualified_at = split_forms(
        forms_at,
        |artifact| pinned.contains_key(artifact),
        &mut bare_at,
    );

    let mut decisions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    // A decision names the function a scenario executed, whichever copy its
    // link took; its copy is not named until coverage locations carry one
    // (), so it cites every copy.
    for directory in DECISIONS {
        for (word, files) in decision_words(&root.join(scanned.verification(directory)))? {
            decisions.entry(word).or_default().extend(files);
        }
    }
    words.extend(decisions.keys().cloned());
    if let Some(rom) = scanned.rom(root)?
        && let Some(summaries) = rom.summaries
    {
        rom_summary_citations(
            summary_names(&root.join(summaries))?,
            &rom.elf,
            &mut qualified,
        );
    }
    let known: BTreeSet<&str> = current
        .keys()
        .map(|(_, _, name)| name.as_str())
        .chain(registry.iter().map(|e| e.symbol.as_str()))
        .collect();
    citations.extend(uncharted_citations(&uncharted, &known));
    let references = words
        .iter()
        .filter(|w| known.contains(w.as_str()))
        .cloned()
        .collect();
    let documents = if pinned.is_empty() {
        vec![]
    } else {
        document_symbols(root, chip, &pinned, &defined)?
    };
    // Only names of pinned or registered functions are decision citations.
    let decisions = decisions
        .into_iter()
        .filter(|(name, _)| known.contains(name.as_str()))
        .collect();
    Ok(Survey {
        documents,
        registry,
        current,
        references,
        qualified,
        qualified_at,
        bare_at,
        words,
        decisions,
        pinned,
        citations,
    })
}

impl Survey {
    /// Whether some citation names this copy: its bare name, or the copy.
    fn cites(&self, artifact: &str, member: &str, symbol: &str) -> bool {
        self.references.contains(symbol)
            || self
                .qualified
                .get(symbol)
                .is_some_and(|copies| copies.contains(&(artifact.to_owned(), member.to_owned())))
    }
}

/// Undefined obfuscated symbols of every document under `docs/vendor/<chip>`.
fn document_symbols(
    root: &Path,
    chip: &str,
    pinned: &BTreeMap<String, (PathBuf, Vec<Function>)>,
    defined: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let directory = root.join("docs/vendor").join(chip);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(vec![]);
    };
    let artifacts = pinned.keys().cloned().collect::<Vec<_>>().join(", ");
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension().and_then(|e| e.to_str()) == Some("md"));
    paths.sort();
    let mut problems = vec![];
    for path in paths {
        let relative = path.strip_prefix(root).unwrap_or(&path);
        problems.extend(docs::undefined_symbols(
            &relative.display().to_string(),
            &std::fs::read_to_string(&path)?,
            defined,
            &artifacts,
        ));
    }
    Ok(problems)
}

/// Every provenance violation of `chip`, one line each.
pub fn violations(root: &Path, chip: &str) -> Result<Vec<String>> {
    Ok(problems_of(&survey(root, chip)?))
}

/// The provenance problems of one survey.
fn problems_of(survey: &Survey) -> Vec<String> {
    let mut problems = survey.citations.clone();
    for (at, artifact, member, symbol) in &survey.qualified_at {
        if !survey
            .current
            .contains_key(&(artifact.clone(), member.clone(), symbol.clone()))
        {
            problems.push(format!(
                "{at}: cites {artifact}[{member}]::{symbol}, which no pinned artifact defines"
            ));
        }
    }
    problems.extend(survey.documents.iter().cloned());
    problems.extend(ambiguous_citations(survey));
    let registered: BTreeSet<(&str, &str, &str)> = survey
        .registry
        .iter()
        .map(|e| (e.artifact.as_str(), e.member.as_str(), e.symbol.as_str()))
        .collect();
    for entry in &survey.registry {
        let key = (
            entry.artifact.clone(),
            entry.member.clone(),
            entry.symbol.clone(),
        );
        if !survey.cites(&entry.artifact, &entry.member, &entry.symbol) {
            problems.push(format!(
                "{}[{}]::{} is registered but no longer cited; `cargo verification provenance --accept {}[{}]::{}` removes it",
                entry.artifact, entry.member, entry.symbol, entry.artifact, entry.member, entry.symbol
            ));
        }
        match survey.current.get(&key) {
            None => problems.push(format!(
                "{}[{}]::{} is cited but absent from the pinned artifact; it was renamed or \
                 removed (see `cargo verification diff`)",
                entry.artifact, entry.member, entry.symbol
            )),
            Some(code) if *code != entry.code => problems.push(format!(
                "{}[{}]::{} changed since its cited facts were reviewed{}",
                entry.artifact,
                entry.member,
                entry.symbol,
                review_hint(survey.decisions.get(&entry.symbol))
            )),
            Some(_) => {}
        }
        let citing: Vec<String> = survey
            .decisions
            .get(&entry.symbol)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        if citing != entry.decisions {
            problems.push(format!(
                "{}::{} is registered for decisions {:?} but cited by {:?}; review them and run \
                 `cargo verification provenance --accept {}`",
                entry.artifact, entry.symbol, entry.decisions, citing, entry.symbol
            ));
        }
    }
    for (artifact, member, symbol) in survey.current.keys() {
        if survey.cites(artifact, member, symbol)
            && !registered.contains(&(artifact.as_str(), member.as_str(), symbol.as_str()))
        {
            problems.push(format!(
                "{artifact}[{member}]::{symbol} is cited but not registered"
            ));
        }
    }
    problems
}

/// The `artifact[member]::symbol` forms `forms_at` wrote whose artifact is
/// pinned, as qualified citations; a form naming any other artifact (such as
/// the file `libpp.a`) cites its symbol bare, into `bare_at`.
fn split_forms(
    forms_at: Vec<(String, String, String, String)>,
    pinned: impl Fn(&str) -> bool,
    bare_at: &mut Vec<(String, BTreeSet<String>)>,
) -> Vec<(String, String, String, String)> {
    let (qualified, unpinned): (Vec<_>, Vec<_>) = forms_at
        .into_iter()
        .partition(|(_, artifact, _, _)| pinned(artifact));
    for (location, _, _, symbol) in unpinned {
        bare_at.push((location, BTreeSet::from([symbol])));
    }
    qualified
}

/// Cite the ROM's copy (`rom_elf`, the chip's pinned ROM ELF) of every
/// function a ROM summary `names`: a summary describes the ROM's code.
fn rom_summary_citations(
    names: BTreeSet<String>,
    rom_elf: &str,
    qualified: &mut BTreeMap<String, BTreeSet<(String, String)>>,
) {
    for name in names {
        qualified
            .entry(name)
            .or_default()
            .insert((rom_elf.to_owned(), String::new()));
    }
}

/// A violation for every bare citation of a function several pinned
/// artifacts define: the fact names one of its copies.
fn ambiguous_citations(survey: &Survey) -> Vec<String> {
    let mut copies: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (artifact, member, symbol) in survey.current.keys() {
        copies
            .entry(symbol.as_str())
            .or_default()
            .push(format!("{artifact}[{member}]::{symbol}"));
    }
    let mut problems = vec![];
    for (location, words) in &survey.bare_at {
        for word in words {
            if let Some(copies) = copies.get(word.as_str()).filter(|c| c.len() > 1) {
                problems.push(format!(
                    "{location}: cites `{word}` bare, but {} pinned copies define it ({}); \
                     cite the copy the fact was read from",
                    copies.len(),
                    copies.join(", ")
                ));
            }
        }
    }
    problems
}

/// What a changed function's reviewer re-reads: the exclusions of every
/// decision file that names it.
fn review_hint(decisions: Option<&BTreeSet<String>>) -> String {
    match decisions {
        Some(files) if !files.is_empty() => {
            let files: Vec<String> = files.iter().map(|f| format!("decisions/{f}")).collect();
            format!("; re-review the exclusions in {}", files.join(", "))
        }
        _ => String::new(),
    }
}

/// The check: fails with every violation.
pub fn check(root: &Path, chip: &str) -> Result<()> {
    let problems = violations(root, chip)?;
    if problems.is_empty() {
        println!(
            "vendor provenance passed: {} cited functions",
            survey_count(root, chip)?
        );
        return Ok(());
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    Err(format!("{} vendor provenance violations", problems.len()).into())
}

fn survey_count(root: &Path, chip: &str) -> Result<usize> {
    Ok(registered(root, chip)?.len())
}

/// The registered functions of `chip`: every vendor function production,
/// the register model or a verification decision cites.
pub fn registered(root: &Path, chip: &str) -> Result<Vec<Entry>> {
    parse_registry(&std::fs::read_to_string(
        root.join(registry_path(root, chip)?),
    )?)
}

/// Prints the annotated pinned code of a cited function.
pub type Show<'a> = dyn Fn(&str) -> Result<()> + 'a;

/// Record reviewed fingerprints. `accept` names the functions whose pinned
/// code was reviewed, and each prints how its fingerprint moves; `show`,
/// when given, prints a cited function's annotated pinned code first (the
/// caller runs the vendor scenarios' `show`). `rebuild` recomputes
/// the whole registry from the current citations, taking fingerprints from
/// the namesake files in `baseline` (the revision the facts were observed
/// in) where present.
pub fn update(
    root: &Path,
    chip: &str,
    accept: &[String],
    rebuild: bool,
    baseline: Option<PathBuf>,
    show: Option<&Show<'_>>,
) -> Result<()> {
    let survey = survey(root, chip)?;
    let decisions_of = |name: &str| -> Vec<String> {
        survey
            .decisions
            .get(name)
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    };
    let mut entries: BTreeSet<Entry> = if rebuild {
        BTreeSet::new()
    } else {
        survey.registry.iter().cloned().collect()
    };
    let current_entries = |symbol: &str| -> Vec<Entry> {
        survey
            .current
            .iter()
            .filter(|((_, _, name), _)| name == symbol)
            .map(|((artifact, member, name), code)| Entry {
                artifact: artifact.clone(),
                member: member.clone(),
                symbol: name.clone(),
                code: code.clone(),
                decisions: decisions_of(name),
            })
            .collect()
    };
    if rebuild {
        let mut baselines: BTreeMap<String, Vec<Function>> = BTreeMap::new();
        if let Some(directory) = &baseline {
            for (artifact, (path, _)) in &survey.pinned {
                let Some(name) = path.file_name() else {
                    continue;
                };
                let old = directory.join(name);
                if old.is_file() {
                    baselines.insert(artifact.clone(), crate::diff::read(&old)?);
                }
            }
        }
        // Cited names only the baseline defines: renamed or removed since
        // their facts were observed.
        for (artifact, functions) in &baselines {
            for f in functions {
                if (survey.words.contains(&f.name) || survey.qualified.contains_key(&f.name))
                    && !survey.current.keys().any(|(_, _, name)| *name == f.name)
                {
                    entries.insert(Entry {
                        artifact: artifact.clone(),
                        member: f.member.clone(),
                        symbol: f.name.clone(),
                        code: f.code.clone(),
                        decisions: decisions_of(&f.name),
                    });
                }
            }
        }
        let cited = survey
            .references
            .iter()
            .chain(survey.qualified.keys())
            .collect::<BTreeSet<_>>();
        for symbol in cited {
            for entry in current_entries(symbol)
                .into_iter()
                .filter(|e| survey.cites(&e.artifact, &e.member, &e.symbol))
            {
                let observed = baselines.get(&entry.artifact).and_then(|functions| {
                    functions
                        .iter()
                        .find(|f| f.name == entry.symbol)
                        .map(|f| f.code.clone())
                });
                // A function the baseline lacks is new: its citation was
                // written against the current revision.
                let code = observed.unwrap_or_else(|| entry.code.clone());
                entries.insert(Entry { code, ..entry });
            }
        }
    }
    for accepted in accept {
        let (place, symbol) = accepted_function(accepted);
        let selected = |e: &Entry| {
            e.symbol == symbol && place.is_none_or(|(a, m)| e.artifact == a && e.member == m)
        };
        let previous: Vec<Entry> = entries.iter().filter(|e| selected(e)).cloned().collect();
        let registered = !previous.is_empty();
        entries.retain(|e| !selected(e));
        let defined: Vec<Entry> = current_entries(symbol)
            .into_iter()
            .filter(|e| selected(e))
            .collect();
        if defined.is_empty() && !registered {
            return Err(
                format!("{accepted}: neither registered nor defined by a pinned artifact").into(),
            );
        }
        let (current, uncited): (Vec<Entry>, Vec<Entry>) = defined
            .into_iter()
            .partition(|e| survey.cites(&e.artifact, &e.member, &e.symbol));
        if current.is_empty() {
            if uncited.is_empty() {
                println!("{accepted}: no pinned definition; its registration is removed");
            } else {
                for e in &uncited {
                    println!(
                        "{}[{}]::{}: no citation names this copy; its registration is removed",
                        e.artifact, e.member, e.symbol
                    );
                }
            }
        }
        if let Some(show) = show
            && !current.is_empty()
        {
            show(symbol)
                .map_err(|error| format!("{accepted}: showing its pinned code failed: {error}"))?;
        }
        for entry in &current {
            println!("{}", fingerprint_move(entry, &previous));
            if !entry.decisions.is_empty() {
                println!(
                    "{symbol}: accepted with the exclusions of {}",
                    entry.decisions.join(", ")
                );
            }
        }
        entries.extend(current);
    }
    let entries: Vec<Entry> = entries.into_iter().collect();
    std::fs::write(
        root.join(registry_path(root, chip)?),
        render_registry(&entries),
    )?;
    println!("{} registered functions", entries.len());
    Ok(())
}

/// How accepting `entry` moves its registered fingerprint, one line.
fn fingerprint_move(entry: &Entry, previous: &[Entry]) -> String {
    let name = format!("{}[{}]::{}", entry.artifact, entry.member, entry.symbol);
    match previous
        .iter()
        .find(|e| e.artifact == entry.artifact && e.member == entry.member)
    {
        None => format!("{name}: newly registered at {}", entry.code),
        Some(old) if old.code == entry.code => format!("{name}: unchanged at {}", entry.code),
        Some(old) => format!("{name}: {} -> {}", old.code, entry.code),
    }
}

/// The function an `--accept` argument names: a bare symbol, or the
/// `artifact[member]::symbol` form the check prints, which narrows it to
/// that artifact member.
fn accepted_function(accepted: &str) -> (Option<(&str, &str)>, &str) {
    let Some((place, symbol)) = accepted.rsplit_once("::") else {
        return (None, accepted);
    };
    match place
        .strip_suffix(']')
        .and_then(|place| place.split_once('['))
    {
        Some((artifact, member)) => (Some((artifact, member)), symbol),
        None => (None, accepted),
    }
}

mod docs;

#[cfg(test)]
mod tests;
