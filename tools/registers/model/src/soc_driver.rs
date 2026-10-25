//! The evidence of an overlay's driver-write exceptions (`driver-writes`,
//! `driver-writes-word`): each cited line of the pinned ESP-IDF driver is
//! opened and must write the named field of the register.
//!
//! A one-line write (`driver-writes`) is either an assignment
//! `<INSTANCE>.<member>.<field> = …` (`|=`, `&=`), where `<INSTANCE>` is a
//! device the layout declares `extern`, `<member>` the device's member of
//! the register's union and `<field>` the union's member at the field's
//! bits, or a register write macro (`WRITE_PERI_REG`, `REG_SET_FIELD`, …)
//! whose first argument is the register's `<NAME>_REG` and whose other
//! arguments name the field's macro (`<FIELD>`, `_M`, `_V`, `_S`), or
//! `HAL_FORCE_MODIFY_U32_REG_FIELD(<INSTANCE>.<member>, <field>, …)`.
//!
//! A word write (`driver-writes-word`) assigns an identifier on its value
//! line and stores the whole word on its store line (`<…>.<member>.val =`,
//! `<…>-><member>.val =`, `WRITE_PERI_REG(<NAME>_REG, …)`, `REG_WRITE`):
//! either the value names the field's macro and the store, in the same
//! function, holds the identifier; or a one-bit field's value is its bit as
//! a literal (`1 << n`, `BIT(n)`, the constant), the function returns the
//! identifier on the cited `return` line, and the store is in the same
//! file. The second form's path from the function's result to the store
//! is not checked: the check lists it as unchecked, and the entry's
//! `reviewed-chain` cites it line by line.

use crate::soc_overlay::{HeaderField, HeaderRegister, HeaderStruct};

/// Macros of ESP-IDF's `soc/soc.h` that write a register: the first
/// argument is the register's address.
const WRITE_MACROS: &[&str] = &[
    "WRITE_PERI_REG",
    "REG_WRITE",
    "SET_PERI_REG_MASK",
    "CLEAR_PERI_REG_MASK",
    "SET_PERI_REG_BITS",
    "REG_SET_BIT",
    "REG_CLR_BIT",
    "REG_SET_BITS",
    "REG_SET_FIELD",
];

/// ESP-IDF's `hal/misc.h` macro that writes one field of a device member:
/// `(<INSTANCE>.<member>, <field>, <value>)`.
const FIELD_MACRO: &str = "HAL_FORCE_MODIFY_U32_REG_FIELD";

/// Macros that store a whole word.
const WORD_MACROS: &[&str] = &["WRITE_PERI_REG", "REG_WRITE"];

/// What the pinned layout says of one register: its union's members and
/// the device members and instances holding it.
pub(crate) struct LayoutRegister<'a> {
    pub(crate) header: &'a HeaderRegister,
    pub(crate) union: Option<&'a HeaderStruct>,
    /// The device's members of the register's union type.
    pub(crate) members: Vec<String>,
    /// The `extern` instances of that device.
    pub(crate) instances: Vec<String>,
}

impl<'a> LayoutRegister<'a> {
    /// The register `header` in the layout text `layout` parsed as
    /// `structs`.
    pub(crate) fn new(
        header: &'a HeaderRegister,
        structs: &'a [HeaderStruct],
        layout: Option<&str>,
    ) -> Self {
        let type_name = format!("{}_reg_t", header.name.to_ascii_lowercase());
        let union = structs.iter().find(|s| s.type_name == type_name);
        let mut members = Vec::new();
        let mut instances = Vec::new();
        if let Some(layout) = layout {
            let mut current = Vec::new();
            let mut devices = Vec::new();
            for line in layout.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("volatile ")
                    && let Some(member) = rest.strip_prefix(&type_name)
                    && member.starts_with(char::is_whitespace)
                {
                    let name = identifier(member.trim_start());
                    if !name.is_empty() {
                        current.push(name.to_owned());
                    }
                } else if trimmed.starts_with('}') && trimmed.ends_with("_dev_t;") {
                    if !current.is_empty() {
                        devices.push(
                            trimmed
                                .trim_start_matches('}')
                                .trim()
                                .trim_end_matches(';')
                                .to_owned(),
                        );
                        members.append(&mut current);
                    }
                    current.clear();
                }
            }
            for line in layout.lines() {
                let words = identifiers(line);
                if let [first, device, instance] = words.as_slice()
                    && *first == "extern"
                    && devices.iter().any(|d| d == device)
                {
                    instances.push((*instance).to_owned());
                }
            }
        }
        Self {
            header,
            union,
            members,
            instances,
        }
    }

    fn register_macro(&self) -> String {
        format!("{}_REG", self.header.name)
    }

    /// The union's member at the bits of `field`.
    fn union_member(&self, field: &HeaderField) -> Option<&str> {
        self.union.and_then(|union| {
            union
                .members
                .iter()
                .find(|(_, low, width)| *low == field.low && *width == field.width())
                .map(|(name, _, _)| name.as_str())
        })
    }
}

fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    text.get(..end).unwrap_or("")
}

/// The identifiers of `line`, outside its `//` comment.
fn identifiers(line: &str) -> Vec<&str> {
    code(line)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
        .collect()
}

/// `line` without its `//` comment.
fn code(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _)| code)
}

/// Whether `rest`, after an lvalue, assigns it (`=`, `|=`, `&=`, not `==`).
fn assigns(rest: &str) -> bool {
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("|=")
        .or_else(|| rest.strip_prefix("&="))
        .map(|_| "")
        .or_else(|| rest.strip_prefix('='));
    rest.is_some_and(|rest| !rest.starts_with('='))
}

/// The positions in `line` where the token `token` starts.
fn token_positions<'l>(line: &'l str, token: &'l str) -> impl Iterator<Item = usize> + 'l {
    line.match_indices(token).filter_map(move |(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + token.len()..].chars().next();
        let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        (boundary(before) && boundary(after)).then_some(at)
    })
}

/// The arguments of the macro call starting at `open` (`(`), split at
/// top-level commas; `None` when the call does not close on the line.
fn arguments(text: &str) -> Option<Vec<&str>> {
    let mut depth = 0_u32;
    let mut start = 1;
    let mut arguments = Vec::new();
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    arguments.push(text[start..at].trim());
                    return Some(arguments);
                }
            }
            ',' if depth == 1 => {
                arguments.push(text[start..at].trim());
                start = at + 1;
            }
            _ => {}
        }
    }
    None
}

/// The arguments of each call of a macro of `macros` on `line` whose first
/// argument is the register's macro, without that first argument.
fn register_macro_calls<'l>(
    line: &'l str,
    register: &LayoutRegister<'_>,
    macros: &'l [&'l str],
) -> Vec<Vec<&'l str>> {
    let line = code(line);
    let address = register.register_macro();
    let mut calls = Vec::new();
    for name in macros {
        for at in token_positions(line, name) {
            let rest = line[at + name.len()..].trim_start();
            if rest.starts_with('(')
                && let Some(arguments) = arguments(rest)
                && arguments.first() == Some(&address.as_str())
            {
                calls.push(arguments.get(1..).unwrap_or_default().to_vec());
            }
        }
    }
    calls
}

/// The field's macros of the pinned header.
fn field_macros(field: &HeaderField) -> [String; 4] {
    let name = &field.name;
    [
        name.clone(),
        format!("{name}_M"),
        format!("{name}_V"),
        format!("{name}_S"),
    ]
}

fn names_field(text: &str, field: &HeaderField) -> bool {
    let macros = field_macros(field);
    identifiers(text)
        .iter()
        .any(|word| macros.iter().any(|name| name == word))
}

/// Whether `line` writes `field` of `register` (a `driver-writes` line).
pub(crate) fn writes_field(line: &str, register: &LayoutRegister<'_>, field: &HeaderField) -> bool {
    let code = code(line);
    if let Some(member) = register.union_member(field) {
        for instance in &register.instances {
            for device_member in &register.members {
                let lvalue = format!("{instance}.{device_member}.{member}");
                if token_positions(code, &lvalue).any(|at| assigns(&code[at + lvalue.len()..])) {
                    return true;
                }
            }
        }
    }
    if let Some(member) = register.union_member(field) {
        for at in token_positions(code, FIELD_MACRO) {
            let rest = code[at + FIELD_MACRO.len()..].trim_start();
            if rest.starts_with('(')
                && let Some(arguments) = arguments(rest)
                && let [device, named, _] = arguments.as_slice()
                && *named == member
                && register.instances.iter().any(|instance| {
                    register
                        .members
                        .iter()
                        .any(|device_member| *device == format!("{instance}.{device_member}"))
                })
            {
                return true;
            }
        }
    }
    register_macro_calls(code, register, WRITE_MACROS)
        .iter()
        .any(|arguments| {
            arguments
                .iter()
                .any(|argument| names_field(argument, field))
        })
}

/// The identifier `line` assigns (`[type] X = …;`) and the assigned
/// expression.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let code = code(line).trim();
    let (left, right) = code.split_once('=')?;
    if right.starts_with('=') || left.ends_with(['!', '<', '>', '|', '&', '+', '-', '*', '/']) {
        return None;
    }
    let target = identifiers(left).into_iter().next_back()?;
    left.trim_end()
        .ends_with(target)
        .then_some((target, right.trim().trim_end_matches(';').trim()))
}

/// Whether `expression` is the one-bit literal of bit `bit`.
fn bit_literal(expression: &str, bit: u32) -> bool {
    let compact: String = expression.chars().filter(|c| !c.is_whitespace()).collect();
    let mut text = compact.as_str();
    while let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        text = inner;
    }
    let number = |text: &str| -> Option<u64> {
        let text = text.trim_end_matches(['u', 'U', 'l', 'L']);
        match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16).ok(),
            None => text.parse().ok(),
        }
    };
    if let Some(argument) = text.strip_prefix("BIT(").and_then(|t| t.strip_suffix(')')) {
        return number(argument) == Some(u64::from(bit));
    }
    if let Some((one, shift)) = text.split_once("<<") {
        return number(one) == Some(1) && number(shift) == Some(u64::from(bit));
    }
    number(text) == Some(1_u64 << bit)
}

/// The brace depth at the start of each line of `text` (comments and
/// literals skipped), one entry per line.
fn depths(text: &str) -> Vec<u32> {
    let mut depth = 0_u32;
    let mut block = false;
    let mut result = Vec::new();
    for line in text.lines() {
        result.push(depth);
        let mut chars = line.chars().peekable();
        let mut quote: Option<char> = None;
        while let Some(c) = chars.next() {
            if block {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    block = false;
                }
                continue;
            }
            if let Some(open) = quote {
                if c == '\\' {
                    chars.next();
                } else if c == open {
                    quote = None;
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'/') => break,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    block = true;
                }
                '"' | '\'' => quote = Some(c),
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    result
}

/// Whether lines `first..=last` (1-based) of `text` lie in one function
/// body: inside braces, which never all close between them.
fn same_function(text: &str, first: usize, last: usize) -> bool {
    let depths = depths(text);
    first <= last
        && first
            .checked_sub(1)
            .and_then(|start| depths.get(start..last))
            .is_some_and(|lines| lines.iter().all(|&depth| depth >= 1))
}

/// Whether `line` stores the whole word of `register`; `value` must then
/// be in the stored value where given.
fn stores_word(line: &str, register: &LayoutRegister<'_>, value: Option<&str>) -> bool {
    let code = code(line);
    let holds =
        |expression: &str| value.is_none_or(|value| identifiers(expression).contains(&value));
    for member in &register.members {
        for separator in [".", "->"] {
            let lvalue = format!("{separator}{member}.val");
            for (at, _) in code.match_indices(&lvalue) {
                let rest = &code[at + lvalue.len()..];
                let boundary = !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_');
                if boundary
                    && assigns(rest)
                    && let Some((_, right)) = rest.split_once('=')
                    && holds(right)
                {
                    return true;
                }
            }
        }
    }
    register_macro_calls(code, register, WORD_MACROS)
        .iter()
        .any(|arguments| arguments.first().is_some_and(|stored| holds(stored)))
}

/// One cited line: its file's text and its 1-based number.
pub(crate) struct Cited<'t> {
    pub(crate) path: &'t str,
    pub(crate) text: &'t str,
    pub(crate) line: usize,
}

impl Cited<'_> {
    fn line(&self) -> Option<&str> {
        self.line
            .checked_sub(1)
            .and_then(|index| self.text.lines().nth(index))
    }
}

/// Why the word write of `field` does not hold, or `None` when it does.
pub(crate) fn word_write_failure(
    register: &LayoutRegister<'_>,
    field: &HeaderField,
    value: &Cited<'_>,
    returned: Option<&Cited<'_>>,
    store: &Cited<'_>,
) -> Option<String> {
    let Some(value_line) = value.line() else {
        return Some(format!(
            "{}:{} is past the file's end",
            value.path, value.line
        ));
    };
    let Some(store_line) = store.line() else {
        return Some(format!(
            "{}:{} is past the file's end",
            store.path, store.line
        ));
    };
    let Some((target, expression)) = assignment(value_line) else {
        return Some(format!(
            "{}:{} assigns no identifier",
            value.path, value.line
        ));
    };
    if store.path != value.path {
        return Some(format!(
            "{}:{} is not in the value's file {}",
            store.path, store.line, value.path
        ));
    }
    match returned {
        None => {
            if !names_field(expression, field) {
                return Some(format!(
                    "{}:{} names no macro of {}",
                    value.path, value.line, field.name
                ));
            }
            if !same_function(
                value.text,
                value.line.min(store.line),
                value.line.max(store.line),
            ) || store.line < value.line
            {
                return Some(format!(
                    "{}:{} is not later in the function of {}:{}",
                    store.path, store.line, value.path, value.line
                ));
            }
            if !stores_word(store_line, register, Some(target)) {
                return Some(format!(
                    "{}:{} stores no word of {} holding {target}",
                    store.path, store.line, register.header.name
                ));
            }
        }
        Some(returned) => {
            if field.width() != 1 || !bit_literal(expression, field.low) {
                return Some(format!(
                    "{}:{} assigns {target} no literal of the one bit {}",
                    value.path, value.line, field.low
                ));
            }
            let Some(return_line) = returned.line() else {
                return Some(format!(
                    "{}:{} is past the file's end",
                    returned.path, returned.line
                ));
            };
            let expected = format!("return {target};");
            if returned.path != value.path
                || code(return_line).trim() != expected
                || returned.line < value.line
                || !same_function(value.text, value.line, returned.line)
            {
                return Some(format!(
                    "{}:{} is not `{expected}` later in the function of {}:{}",
                    returned.path, returned.line, value.path, value.line
                ));
            }
            if !stores_word(store_line, register, None) {
                return Some(format!(
                    "{}:{} stores no word of {}",
                    store.path, store.line, register.header.name
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests;
