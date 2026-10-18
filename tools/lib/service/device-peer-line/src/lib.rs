//! The one text protocol of the stand's reference peers (`hil/peers/*`) on
//! their USB Serial/JTAG console: `@READY protocol=N key=value...`, `@OK
//! <command>`, `@ERR <command> <reason>` and `@<REPORT> fields...` lines,
//! and the hex encoding of their byte fields.
#![forbid(unsafe_code)]

/// One line of the peer protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Line {
    /// `@READY protocol=N ...`: the peer is ready, speaking protocol `N`;
    /// its other `key=value` fields describe the image.
    Ready { protocol: u32, report: Report },
    /// `@OK <command>`.
    Ok { command: String },
    /// `@ERR <command> <reason...>`.
    Err { command: String, reason: String },
    /// Any other `@<NAME> fields...` line.
    Report(Report),
}

/// A report line: its name and its space-separated fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub name: String,
    pub fields: Vec<String>,
}

impl Report {
    /// The `index`th field.
    pub fn positional(&self, index: usize) -> Option<&str> {
        self.fields.get(index).map(String::as_str)
    }

    /// The value of the first `key=value` field.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find_map(|field| field.strip_prefix(key)?.strip_prefix('='))
    }

    /// The value of `key=value`, parsed.
    pub fn parsed<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.field(key)?.parse().ok()
    }

    /// The `0`/`1` flag `key=value`.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.field(key)? {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        }
    }
}

/// Parse one console line; a line without the `@` prefix is no protocol
/// line, and neither is a known answer missing its command.
pub fn parse(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']).strip_prefix('@')?;
    let mut words = line.split(' ');
    let name = words.next().filter(|name| !name.is_empty())?;
    let fields: Vec<String> = words.map(str::to_owned).collect();
    match name {
        "OK" => Some(Line::Ok {
            command: fields.first()?.clone(),
        }),
        "ERR" => Some(Line::Err {
            command: fields.first()?.clone(),
            reason: fields.get(1..)?.join(" "),
        }),
        _ => {
            let report = Report {
                name: name.to_owned(),
                fields,
            };
            match name {
                "READY" => Some(Line::Ready {
                    protocol: report.parsed("protocol")?,
                    report,
                }),
                _ => Some(Line::Report(report)),
            }
        }
    }
}

/// Bytes from lowercase or uppercase hex digits, two per byte.
pub fn hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

/// Lowercase hex digits of `bytes`, as the peers read them.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
