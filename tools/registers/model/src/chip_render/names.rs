//! Rust names of the model's entities: the block prefix rule, snake and
//! camel case, keywords.

use crate::chip_model::BlockType;

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while",
];

/// The vendor prefix of a block type's names: the longest run of whole
/// `_`-separated segments every named (not `_OPAQUE`) top-level register
/// and group of the type starts with, followed by `_`. A type with fewer
/// than two such names, or whose names share no segment, has none. The
/// prefix never holds `%s` and is shorter than every name it is cut from.
pub(super) fn block_prefix(block: &BlockType) -> String {
    let names = block
        .file
        .registers
        .iter()
        .map(|register| register.name.as_str())
        .chain(block.file.groups.iter().map(|group| group.name.as_str()))
        .filter(|name| !name.ends_with("_OPAQUE"))
        .map(|name| name.split('_').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let [first, rest @ ..] = names.as_slice() else {
        return String::new();
    };
    if rest.is_empty() {
        return String::new();
    }
    let shortest = names.iter().map(Vec::len).min().unwrap_or(0);
    let mut common = 0;
    while common + 1 < shortest {
        let Some(segment) = first.get(common) else {
            break;
        };
        if segment.is_empty()
            || segment.contains('%')
            || rest.iter().any(|name| name.get(common) != Some(segment))
        {
            break;
        }
        common += 1;
    }
    if common == 0 {
        return String::new();
    }
    let mut prefix = first.get(..common).unwrap_or_default().join("_");
    prefix.push('_');
    prefix
}

/// `name` without the block prefix, unless what is left would be empty or
/// start with a digit: then the name keeps its prefix.
pub(super) fn strip<'a>(name: &'a str, prefix: &str) -> &'a str {
    match name.strip_prefix(prefix) {
        Some(rest)
            if !prefix.is_empty()
                && !rest.is_empty()
                && !rest.starts_with(|character: char| character.is_ascii_digit()) =>
        {
            rest
        }
        _ => name,
    }
}

/// The lower snake case of a model name, `%s` removed, without keyword
/// escaping: the stem of method and module names.
pub(super) fn snake(name: &str) -> String {
    let mut output = String::new();
    for character in name.replace("%s", "").chars() {
        let character = match character {
            '.' | '-' => '_',
            other => other.to_ascii_lowercase(),
        };
        if character == '_' && (output.is_empty() || output.ends_with('_')) {
            continue;
        }
        output.push(character);
    }
    while output.ends_with('_') {
        output.pop();
    }
    output
}

/// A snake-case identifier: `r#` before a keyword.
pub(super) fn ident(snake: &str) -> String {
    if KEYWORDS.contains(&snake) {
        format!("r#{snake}")
    } else {
        snake.to_owned()
    }
}

/// The upper camel case of a model name, `%s` removed.
pub(super) fn camel(name: &str) -> String {
    crate::pac_api_render::type_binding_name(&name.replace('.', "_"))
}

/// Whether `name` is a usable Rust identifier stem.
pub(super) fn valid(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|character: char| character.is_ascii_digit())
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}
