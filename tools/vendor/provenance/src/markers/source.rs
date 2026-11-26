//! `SOURCE:` recovered-fact comment blocks and the chips they belong to.
//!
//! Production records a recovered vendor fact in a comment block opened by
//! the marker `SOURCE`. The marker may name the chips whose pinned artifacts
//! the fact describes, in parentheses directly after it:
//!
//! ```text
//! // SOURCE(<chip>): complete `libpp.a[trc.o]::rcGetRate` …
//! /// SOURCE(<chip>,<chip>)[EVIDENCE_ID]: …
//! ```
//!
//! Any bracketed evidence tag keeps following the marker unchanged. A block
//! under a chip's own directory belongs to that chip and may omit the list;
//! a block under a chip-neutral path that cites a vendor function must name
//! its chips. This module is the one recogniser and parser of that grammar:
//! the provenance check reads blocks with it, and [`is_marker_line`] finds
//! markers in a file's lines.
use std::path::Path;

/// The marker that opens a recovered-fact comment block.
pub const MARKER: &str = "SOURCE";

/// One recovered-fact comment block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    /// One-based line of the marker.
    pub line: usize,
    /// The chips the marker names, when it names any.
    pub chips: Option<Vec<String>>,
    /// The block's comment text, without the chip list.
    pub text: String,
}

/// A marker whose chip list does not parse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MalformedMarker {
    pub line: usize,
    pub reason: &'static str,
}

/// The comment syntax of a scanned file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Syntax {
    /// `//`, `/*` and `*` lines; `#` begins an attribute.
    Rust,
    /// `#` lines, such as a TOML file's.
    Hash,
    /// Markdown prose: a block runs from its marker to the end of the
    /// paragraph.
    Markdown,
}

impl Syntax {
    /// The syntax of a file named `name`, if it is scanned.
    pub fn of(name: &str) -> Option<Self> {
        if name.ends_with(".rs") {
            Some(Self::Rust)
        } else if name.ends_with(".toml") {
            Some(Self::Hash)
        } else if name.ends_with(".md") {
            Some(Self::Markdown)
        } else {
            None
        }
    }

    /// Whether `line` is a comment line of this syntax.
    pub fn is_comment(self, line: &str) -> bool {
        let line = line.trim_start();
        match self {
            Self::Rust => line.starts_with("//") || line.starts_with("/*") || line.starts_with('*'),
            Self::Hash => line.starts_with('#'),
            Self::Markdown => !line.is_empty(),
        }
    }
}

/// Where the marker sits on `line`: the word `SOURCE`, not the end of a
/// longer identifier, directly followed by `:`, `(` or `[` (written `\[` in
/// rustdoc, which would otherwise read the tag as a link). `RESOURCE:` and
/// `SOURCES.md` are no markers.
pub fn marker(line: &str) -> Option<usize> {
    around_marker(line).map(|(before, _)| before.len())
}

/// The text of `line` before its marker and the text after it.
fn around_marker(line: &str) -> Option<(&str, &str)> {
    line.match_indices(MARKER).find_map(|(at, _)| {
        let (before, rest) = line.split_at_checked(at)?;
        let after = rest.strip_prefix(MARKER)?;
        let word = before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        (!word && (after.starts_with([':', '(', '[']) || after.starts_with("\\[")))
            .then_some((before, after))
    })
}

/// Whether `line` is a comment of `syntax` that opens a recovered-fact
/// block.
pub fn is_marker_line(line: &str, syntax: Syntax) -> bool {
    syntax.is_comment(line) && marker(line).is_some()
}

/// The chip list after the marker on `line`, and the line without it.
fn marker_chips(line: &str) -> Result<(Option<Vec<String>>, String), &'static str> {
    let Some((before, list)) =
        around_marker(line).and_then(|(before, after)| Some((before, after.strip_prefix('(')?)))
    else {
        return Ok((None, line.to_owned()));
    };
    let (chips, rest) = list.split_once(')').ok_or("unclosed chip list")?;
    let chips: Vec<String> = chips
        .split(',')
        .map(|chip| chip.trim().to_owned())
        .collect();
    if chips.iter().any(String::is_empty) {
        return Err("empty chip name");
    }
    Ok((Some(chips), format!("{before}{MARKER}{rest}")))
}

/// Every recovered-fact comment block of Rust `text`, or the first
/// malformed marker.
pub fn blocks(text: &str) -> Result<Vec<Block>, MalformedMarker> {
    blocks_in(text, Syntax::Rust)
}

/// Every recovered-fact comment block of `text` in `syntax`, or the first
/// malformed marker.
pub fn blocks_in(text: &str, syntax: Syntax) -> Result<Vec<Block>, MalformedMarker> {
    let mut blocks: Vec<Block> = vec![];
    let mut lines = (1..).zip(text.lines()).peekable();
    while let Some((number, line)) = lines.next() {
        if !is_marker_line(line, syntax) {
            continue;
        }
        let (chips, mut text) = marker_chips(line).map_err(|reason| MalformedMarker {
            line: number,
            reason,
        })?;
        // The block runs on through the comment lines that open no block of
        // their own.
        while let Some((_, more)) =
            lines.next_if(|(_, more)| syntax.is_comment(more) && marker(more).is_none())
        {
            text.push('\n');
            text.push_str(more);
        }
        blocks.push(Block {
            line: number,
            chips,
            text,
        });
    }
    Ok(blocks)
}

/// Where a block lies relative to the scanned chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// Under the scanned chip's own directory.
    OwnChip,
    /// Under no chip's directory.
    Neutral,
}

/// Whether a block cites the scanned `chip`'s pins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Attribution {
    Cited,
    NotCited,
    /// A chip-neutral block that names no chip: a violation when it cites a
    /// vendor function, since its chip is then unknown.
    Uncharted,
    /// A violation, with its reason.
    Invalid(String),
}

/// The attribution of `block` at `place` for a scan of `chip`, whose names
/// must be among `known`.
pub fn attribute(block: &Block, place: Place, chip: &str, known: &[String]) -> Attribution {
    if let Some(unknown) = block.chips.iter().flatten().find(|c| !known.contains(c)) {
        return Attribution::Invalid(format!("unknown chip `{unknown}`"));
    }
    match (place, &block.chips) {
        (Place::OwnChip, None) => Attribution::Cited,
        (Place::OwnChip, Some(chips)) if chips.iter().any(|c| c == chip) => Attribution::Cited,
        (Place::OwnChip, Some(_)) => {
            Attribution::Invalid(format!("names other chips under the {chip} directory"))
        }
        (Place::Neutral, None) => Attribution::Uncharted,
        (Place::Neutral, Some(chips)) if chips.iter().any(|c| c == chip) => Attribution::Cited,
        (Place::Neutral, Some(_)) => Attribution::NotCited,
    }
}

/// The place of `path` for a scan of `chip`, or `None` under another
/// supported chip's directory.
pub fn place(path: &Path, chip: &str, supported: &[String]) -> Option<Place> {
    let mut own = false;
    for component in path.components() {
        let component = component.as_os_str().to_str().unwrap_or_default();
        if component == chip {
            own = true;
        } else if supported.iter().any(|c| c == component) {
            return None;
        }
    }
    Some(if own { Place::OwnChip } else { Place::Neutral })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec!["chip-b".into(), "chip-a".into()]
    }

    #[test]
    fn a_marker_names_its_chips_before_any_evidence_tag() {
        let text = "// SOURCE(chip-a, chip-b)[TAG]: `phy_i2c_init1`\n\
                    // continues `phy_rf_init`\nfn x() {}\n\
                    /// SOURCE: plain\n";
        let blocks = blocks(text).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].chips,
            Some(vec!["chip-a".to_owned(), "chip-b".to_owned()])
        );
        assert!(blocks[0].text.contains("SOURCE[TAG]"));
        assert!(!blocks[0].text.contains("chip-b"));
        assert!(blocks[0].text.contains("phy_rf_init"));
        assert_eq!(blocks[1].chips, None);
        assert_eq!(blocks[1].line, 4);
    }

    #[test]
    fn a_markdown_block_runs_to_the_end_of_its_paragraph() {
        let text = "Prose before.\nThe walker reads LAST. SOURCE: ROM_X\n`rom[]::f` continues\n\nnext `g`\n";
        assert_eq!(Syntax::of("note.md"), Some(Syntax::Markdown));
        let blocks = blocks_in(text, Syntax::Markdown).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line, 2);
        assert!(blocks[0].text.contains("rom[]::f"));
        assert!(!blocks[0].text.contains("`g`"));
    }

    #[test]
    fn hash_comments_carry_blocks_in_toml_but_not_in_rust() {
        let text = "# SOURCE(chip-a): rev0 ROM `memset`\n# never writes `sp`\nname = 1\n";
        let toml = blocks_in(text, Syntax::Hash).unwrap();
        assert_eq!(toml.len(), 1);
        assert!(toml[0].text.contains("never writes"));
        // In Rust a `#` line is an attribute and ends a block.
        let rust = "// SOURCE(chip-a): `memset`\n#[inline]\n// unrelated\nfn f() {}\n";
        let block = &blocks(rust).unwrap()[0];
        assert!(!block.text.contains("unrelated"));
        assert_eq!(Syntax::of("functions.toml"), Some(Syntax::Hash));
        assert_eq!(Syntax::of("lib.rs"), Some(Syntax::Rust));
        assert_eq!(Syntax::of("lib.c"), None);
    }

    #[test]
    fn only_the_word_source_followed_by_its_punctuation_is_a_marker() {
        assert_eq!(marker("// SOURCE: x"), Some(3));
        assert_eq!(
            marker("// see RESOURCE: x, then SOURCE(chip-a): y"),
            Some(25)
        );
        assert_eq!(marker("/// SOURCE[TAG]: x"), Some(4));
        assert_eq!(marker("/// SOURCE\\[TAG]: x"), Some(4));
        for line in [
            "// see SOURCES.md",
            "// RESOURCE: x",
            "// SOURCE x",
            "// MY_SOURCE: x",
        ] {
            assert_eq!(marker(line), None, "{line}");
        }
        assert!(is_marker_line("  // SOURCE(chip-b): x", Syntax::Rust));
        assert!(!is_marker_line("let s = \"SOURCE: x\";", Syntax::Rust));
        assert!(is_marker_line("# SOURCE: rom", Syntax::Hash));
        // A prose line naming source files opens no block.
        let text = "// see SOURCES.md for the list\n// `phy_x` is not cited\n";
        assert!(blocks(text).unwrap().is_empty());
    }

    #[test]
    fn a_marker_is_found_past_the_words_that_only_hold_it() {
        // The second of two that touch follows a letter of the first.
        assert_eq!(marker("// SOURCESOURCE: x"), None);
        assert_eq!(marker("// SOURCE SOURCE: x"), Some(10));
        assert_eq!(marker("// RESOURCE: SOURCE x SOURCE[T]: y"), Some(22));
        // The offset is in bytes, after text of any width.
        assert_eq!(marker("// \u{e9}t\u{e9} \u{2014}SOURCE: x"), Some(12));
        assert_eq!(marker("SOURCE: x"), Some(0));
        assert_eq!(marker("// SOURCE"), None);
        assert_eq!(marker(""), None);
    }

    #[test]
    fn a_block_is_its_marker_line_and_the_comment_lines_that_follow_it() {
        let text = "// before any marker\n\
                    // SOURCE(chip-a)[TAG]: first\n\
                    // SOURCE: second, directly after\n\
                    // its continuation\n\
                    fn x() {}\n\
                    // not a continuation\n\
                    \n\
                    // SOURCE( chip-a ,chip-b ): third, the last line";
        assert_eq!(
            blocks(text).unwrap(),
            [
                Block {
                    line: 2,
                    chips: Some(vec!["chip-a".to_owned()]),
                    text: "// SOURCE[TAG]: first".to_owned(),
                },
                Block {
                    line: 3,
                    chips: None,
                    text: "// SOURCE: second, directly after\n// its continuation".to_owned(),
                },
                Block {
                    line: 8,
                    chips: Some(vec!["chip-a".to_owned(), "chip-b".to_owned()]),
                    text: "// SOURCE: third, the last line".to_owned(),
                },
            ]
        );
        // A second marker on the line is text; a list that is not directly
        // after the marker is no list.
        assert_eq!(
            blocks("// SOURCE(chip-a): see SOURCE(chip-b): x\n// SOURCE: (chip-a) y").unwrap(),
            [
                Block {
                    line: 1,
                    chips: Some(vec!["chip-a".to_owned()]),
                    text: "// SOURCE: see SOURCE(chip-b): x".to_owned(),
                },
                Block {
                    line: 2,
                    chips: None,
                    text: "// SOURCE: (chip-a) y".to_owned(),
                },
            ]
        );
        assert_eq!(
            blocks("// SOURCE(): x"),
            Err(MalformedMarker {
                line: 1,
                reason: "empty chip name"
            })
        );
        assert_eq!(
            blocks("// SOURCE: fine\n\n// SOURCE(chip-a"),
            Err(MalformedMarker {
                line: 3,
                reason: "unclosed chip list"
            })
        );
    }

    #[test]
    fn a_malformed_chip_list_is_rejected_with_its_line() {
        assert_eq!(
            blocks("fn x() {}\n// SOURCE(chip-a: open\n"),
            Err(MalformedMarker {
                line: 2,
                reason: "unclosed chip list"
            })
        );
        assert!(blocks("// SOURCE(chip-a,): x\n").is_err());
    }

    #[test]
    fn neutral_blocks_must_name_their_chips() {
        let neutral = |chips: Option<Vec<&str>>| Block {
            line: 1,
            chips: chips.map(|c| c.into_iter().map(str::to_owned).collect()),
            text: String::new(),
        };
        let known = known();
        assert_eq!(
            attribute(
                &neutral(Some(vec!["chip-a"])),
                Place::Neutral,
                "chip-a",
                &known
            ),
            Attribution::Cited
        );
        assert_eq!(
            attribute(
                &neutral(Some(vec!["chip-a"])),
                Place::Neutral,
                "chip-b",
                &known
            ),
            Attribution::NotCited
        );
        assert_eq!(
            attribute(&neutral(None), Place::Neutral, "chip-b", &known),
            Attribution::Uncharted
        );
        assert_eq!(
            attribute(&neutral(None), Place::OwnChip, "chip-b", &known),
            Attribution::Cited
        );
        assert!(matches!(
            attribute(
                &neutral(Some(vec!["chip-a"])),
                Place::OwnChip,
                "chip-b",
                &known
            ),
            Attribution::Invalid(_)
        ));
        assert!(matches!(
            attribute(
                &neutral(Some(vec!["esp32"])),
                Place::Neutral,
                "chip-b",
                &known
            ),
            Attribution::Invalid(_)
        ));
    }

    #[test]
    fn a_path_is_own_neutral_or_another_chips() {
        let known = known();
        assert_eq!(
            place(
                Path::new("crates/hardware/chip-b/pac/src/a.rs"),
                "chip-b",
                &known
            ),
            Some(Place::OwnChip)
        );
        assert_eq!(
            place(
                Path::new("crates/protocols/ieee80211/src/a.rs"),
                "chip-b",
                &known
            ),
            Some(Place::Neutral)
        );
        assert_eq!(
            place(
                Path::new("crates/hardware/chip-a/hal/src/a.rs"),
                "chip-b",
                &known
            ),
            None
        );
    }
}
