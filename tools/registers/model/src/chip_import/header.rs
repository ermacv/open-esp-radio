//! The pinned ESP-IDF files the import reads: `*_reg.h` register headers
//! in both of their formats, the `reg_base.h` base macros and the
//! `interrupts.h` source enumeration.

use std::collections::BTreeMap;

/// One field line of a register header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HeaderField {
    pub(super) line: usize,
    pub(super) name: String,
    pub(super) access: String,
    pub(super) low: u32,
    pub(super) width: u32,
    /// The stated default, if the line states one this parser reads.
    pub(super) default: Option<u64>,
    /// The default text when it is not a number this parser reads.
    pub(super) unread_default: Option<String>,
    /// The field's comment, joined by single spaces.
    pub(super) description: String,
}

/// One `#define <NAME>_REG (<base> + 0x<offset>)` and its fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HeaderRegister {
    pub(super) line: usize,
    /// The macro name without its `_REG` suffix.
    pub(super) name: String,
    pub(super) offset: u32,
    pub(super) description: Option<String>,
    pub(super) fields: Vec<HeaderField>,
}

fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    text.get(..end).unwrap_or("")
}

/// A C or Verilog integer literal: `12`, `0x1f`, `12U`, `1'b1`, `32'h0`,
/// `8'd10`.
pub(super) fn parse_literal(text: &str) -> Option<u64> {
    let text = text.trim().replace('_', "");
    if let Some((_, rest)) = text.split_once('\'') {
        let (radix, digits) = rest.split_at_checked(1)?;
        let radix = match radix {
            "b" | "B" => 2,
            "o" | "O" => 8,
            "d" | "D" => 10,
            "h" | "H" => 16,
            _ => return None,
        };
        return u64::from_str_radix(digits.trim(), radix).ok();
    }
    let text = text.trim_end_matches(['U', 'u', 'L', 'l']);
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

/// `bitpos: [<hi>:<lo>]` or `bitpos:[<n>]` as (low, width).
fn parse_bitpos(text: &str) -> Option<(u32, u32)> {
    let rest = text.split_once("bitpos:")?.1.trim_start();
    let inner = rest.strip_prefix('[')?.split_once(']')?.0;
    let (high, low) = inner.split_once(':').unwrap_or((inner, inner));
    let high = u32::try_from(parse_literal(high)?).ok()?;
    let low = u32::try_from(parse_literal(low)?).ok()?;
    (high >= low && high < 32).then(|| (low, high - low + 1))
}

/// The text after `default:` up to its `;`.
fn default_text(text: &str) -> Option<&str> {
    let rest = text.split_once("default:")?.1;
    Some(rest.split(';').next().unwrap_or(rest).trim())
}

/// A field line: `/** NAME : ACCESS; bitpos: [h:l]; default: N;` (the
/// current format) or `/* NAME : ACCESS ;bitpos:[h:l] ;default: 1'b0 ; */`
/// (the older one, whose description follows on its own comment).
fn parse_field_line(line: &str, number: usize) -> Option<(HeaderField, bool)> {
    let trimmed = line.trim_start();
    let (rest, current) = if let Some(rest) = trimmed.strip_prefix("/** ") {
        (rest, true)
    } else {
        (trimmed.strip_prefix("/* ")?, false)
    };
    let name = identifier(rest);
    if name.is_empty() {
        return None;
    }
    let tail = rest.get(name.len()..)?.trim_start().strip_prefix(':')?;
    let (access, tail) = tail.split_once(';')?;
    let (low, width) = parse_bitpos(tail)?;
    let (default, unread_default) = match default_text(tail) {
        None => (None, None),
        Some(text) => match parse_literal(text) {
            Some(value) => (Some(value), None),
            None => (None, Some(text.to_owned())),
        },
    };
    Some((
        HeaderField {
            line: number,
            name: name.to_owned(),
            access: access.trim().to_owned(),
            low,
            width,
            default,
            unread_default,
            description: String::new(),
        },
        current,
    ))
}

fn comment_text(line: &str) -> &str {
    line.trim()
        .trim_start_matches("/**")
        .trim_start_matches("/*")
        .trim_start_matches('*')
        .trim_end_matches("*/")
        .trim()
}

fn append(target: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(text);
}

/// What the line after a field line continues.
enum Open {
    None,
    /// The comment of a current-format field, until `*/`.
    Field,
    /// An older format's `/*description: ... */`, until `*/`.
    OldDescription,
    /// A `/** <NAME>_REG register` comment, until `*/`.
    Register(String),
}

/// Every register of a `*_reg.h`, in file order, with its fields.
pub(super) fn parse_registers(text: &str) -> Vec<HeaderRegister> {
    let mut registers: Vec<HeaderRegister> = Vec::new();
    let mut open = Open::None;
    let mut pending: Option<String> = None;
    // An older-format field waits for its `/*description:` line.
    let mut old_field = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        match &mut open {
            Open::Field => {
                if let Some(field) = registers.last_mut().and_then(|r| r.fields.last_mut())
                    && !line.trim_start().starts_with("*/")
                {
                    let mut text = comment_text(line);
                    // A default wrapped onto the comment's next line.
                    if field.default.is_none()
                        && field.unread_default.as_deref() == Some("")
                        && let Some((value, rest)) = text.split_once(';')
                    {
                        match parse_literal(value) {
                            Some(value) => {
                                field.default = Some(value);
                                field.unread_default = None;
                            }
                            None => field.unread_default = Some(value.trim().to_owned()),
                        }
                        text = rest.trim();
                    }
                    append(&mut field.description, text);
                }
                if line.contains("*/") {
                    open = Open::None;
                }
                continue;
            }
            Open::OldDescription => {
                if let Some(field) = registers.last_mut().and_then(|r| r.fields.last_mut()) {
                    append(&mut field.description, comment_text(line));
                }
                if line.contains("*/") {
                    open = Open::None;
                }
                continue;
            }
            Open::Register(description) => {
                if !line.trim_start().starts_with("*/") {
                    append(description, comment_text(line));
                }
                if line.contains("*/") {
                    let description = std::mem::take(description);
                    pending = (!description.is_empty()).then_some(description);
                    open = Open::None;
                }
                continue;
            }
            Open::None => {}
        }
        let trimmed = line.trim_start();
        if old_field && let Some(rest) = trimmed.strip_prefix("/*description:") {
            old_field = false;
            if let Some(field) = registers.last_mut().and_then(|r| r.fields.last_mut()) {
                let text = rest.trim().trim_end_matches("*/").trim();
                append(&mut field.description, text);
            }
            if !line.contains("*/") {
                open = Open::OldDescription;
            }
            continue;
        }
        old_field = false;
        if let Some(rest) = trimmed.strip_prefix("/** ")
            && let Some(name) = rest.trim_end().strip_suffix(" register")
            && identifier(name) == name
        {
            open = Open::Register(String::new());
            continue;
        }
        if let Some(rest) = line.strip_prefix("#define ") {
            let name = identifier(rest);
            let Some(base) = name.strip_suffix("_REG") else {
                continue;
            };
            let offset = rest
                .split_once("+ 0x")
                .or_else(|| rest.split_once("+0x"))
                .map(|(_, tail)| identifier(tail))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok());
            if let Some(offset) = offset {
                registers.push(HeaderRegister {
                    line: number,
                    name: base.to_owned(),
                    offset,
                    description: pending.take(),
                    fields: Vec::new(),
                });
            }
            continue;
        }
        if let Some((field, current)) = parse_field_line(line, number)
            && let Some(register) = registers.last_mut()
        {
            register.fields.push(field);
            if current {
                if !line.contains("*/") {
                    open = Open::Field;
                }
            } else {
                old_field = true;
            }
        }
    }
    registers
}

/// The `#define <NAME> <expression>` base macros of `reg_base.h` files:
/// each a literal, another macro or a sum of them.
#[derive(Debug, Default)]
pub(super) struct Bases {
    /// The macro's expression and where it is defined (file, line).
    macros: BTreeMap<String, (String, String, usize)>,
}

impl Bases {
    pub(super) fn add(&mut self, path: &str, text: &str) {
        for (index, line) in text.lines().enumerate() {
            let Some(rest) = line.strip_prefix("#define ") else {
                continue;
            };
            let name = identifier(rest);
            let Some(expression) = rest.get(name.len()..) else {
                continue;
            };
            // A function-like macro is no base.
            if name.is_empty() || expression.starts_with('(') {
                continue;
            }
            let expression = expression.split("//").next().unwrap_or("").trim();
            if expression.is_empty() {
                continue;
            }
            self.macros.entry(name.to_owned()).or_insert((
                expression.to_owned(),
                path.to_owned(),
                index + 1,
            ));
        }
    }

    /// The value of `name` and the file and line that define it.
    pub(super) fn resolve(&self, name: &str) -> Option<(u64, String, usize)> {
        let (_, path, line) = self.macros.get(name)?;
        Some((self.value(name, 0)?, path.clone(), *line))
    }

    fn value(&self, name: &str, depth: u32) -> Option<u64> {
        if depth > 16 {
            return None;
        }
        let (expression, _, _) = self.macros.get(name)?;
        let inner = expression.replace(['(', ')'], " ");
        let mut sum = 0_u64;
        for term in inner.split('+') {
            let term = term.trim();
            let value = match parse_literal(term) {
                Some(value) => value,
                None if identifier(term) == term && !term.is_empty() => {
                    self.value(term, depth + 1)?
                }
                None => return None,
            };
            sum = sum.checked_add(value)?;
        }
        Some(sum)
    }
}

/// One source of the interrupt enumeration: its number, its name without
/// `ETS_` and `_INTR_SOURCE`, and its line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InterruptSource {
    pub(super) number: u16,
    pub(super) name: String,
    pub(super) line: usize,
}

/// The sources of `interrupts.h`'s `periph_interrupt_t`, up to
/// `ETS_MAX_INTR_SOURCE`.
pub(super) fn parse_interrupts(text: &str) -> Vec<InterruptSource> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut next = 0_u64;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if !inside {
            inside = trimmed.starts_with("typedef enum");
            continue;
        }
        if trimmed.starts_with('}') {
            break;
        }
        let entry = trimmed.split("//").next().unwrap_or("").trim();
        let entry = entry.trim_end_matches(',').trim();
        let (name, value) = match entry.split_once('=') {
            Some((name, value)) => (name.trim(), parse_literal(value)),
            None => (entry, None),
        };
        if identifier(name) != name || name.is_empty() {
            continue;
        }
        if let Some(value) = value {
            next = value;
        }
        if name == "ETS_MAX_INTR_SOURCE" {
            break;
        }
        let Some(short) = name
            .strip_prefix("ETS_")
            .and_then(|rest| rest.strip_suffix("_INTR_SOURCE"))
        else {
            next += 1;
            continue;
        };
        if let Ok(number) = u16::try_from(next) {
            out.push(InterruptSource {
                number,
                name: short.to_owned(),
                line: index + 1,
            });
        }
        next += 1;
    }
    out
}
