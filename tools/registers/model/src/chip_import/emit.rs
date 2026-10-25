//! The import's files as TOML text: the type files, the chip root and the
//! evidence catalog of every source they cite.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::{
    Cite, Cites, ImportError, ImportSpec, OutInstance, OutInterrupt, OutRegister, OutType, Pin,
    PinnedFiles, Review,
};

/// The file name of type `name`: `HP_SYS_CLKRST` is `hp-sys-clkrst.toml`.
pub fn type_file_name(name: &str) -> String {
    format!("{}.toml", name.to_ascii_lowercase().replace('_', "-"))
}

/// A TOML basic string.
fn string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|item| string(item))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn bits(low: u32, width: u32) -> String {
    if width == 1 {
        low.to_string()
    } else {
        format!("{}:{low}", low + width - 1)
    }
}

/// An evidence ID: upper-case letters, digits and `_`.
fn id_part(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_owned()
}

fn file_id(source: &str, revision: &str, path: &str) -> String {
    let short = revision.get(..8).unwrap_or(revision);
    let path = path.strip_prefix("components/").unwrap_or(path);
    format!("{}_{}_{}", id_part(source), id_part(short), id_part(path))
}

/// The evidence sources the written files cite, collected while writing.
pub(super) struct Catalog {
    svd: Pin,
    esp_idf: String,
    svd_used: bool,
    files: BTreeSet<String>,
    lines: BTreeSet<(String, usize)>,
    /// The revision of the ESP-IDF pin.
    revision: String,
}

impl Catalog {
    pub(super) fn new(svd: Pin, esp_idf: String, revision: String) -> Self {
        Self {
            svd,
            esp_idf,
            svd_used: false,
            files: BTreeSet::new(),
            lines: BTreeSet::new(),
            revision,
        }
    }

    fn svd_id(&self) -> String {
        file_id(&self.svd.source, &self.svd.revision, &self.svd.path)
    }

    fn idf_id(&self, path: &str) -> String {
        file_id(&self.esp_idf, &self.revision, path)
    }

    /// The IDs of `cites`: `headers` stand for [`Cite::Header`].
    fn ids(&mut self, cites: &Cites, headers: &[String]) -> Vec<String> {
        let mut ids = BTreeSet::new();
        for cite in cites {
            match cite {
                Cite::Svd => {
                    self.svd_used = true;
                    ids.insert(self.svd_id());
                }
                Cite::Header => {
                    for path in headers {
                        self.files.insert(path.clone());
                        ids.insert(self.idf_id(path));
                    }
                }
                Cite::Line { path, line } => {
                    self.lines.insert((path.clone(), *line));
                    ids.insert(format!("{}_L{line}", self.idf_id(path)));
                }
            }
        }
        ids.into_iter().collect()
    }

    /// The catalog: every cited file and line, each checked against its
    /// pinned file.
    pub(super) fn render(&self, pinned: &dyn PinnedFiles) -> Result<String, ImportError> {
        let mut out = String::from(
            "# Evidence of the imported register blocks: the pinned vendor files\n\
             # `cargo registers import` read, and each ESP-IDF line a carried-over\n\
             # fact cites.\n\
             schema = 1\n",
        );
        let mut entries = BTreeMap::new();
        if self.svd_used {
            let pin = &self.svd;
            entries.insert(
                self.svd_id(),
                format!(
                    "Vendor SVD pinned in artifacts.toml: {} at {}, {}, sha256 {}, Apache-2.0 (its licenseText). The import takes register and field names, offsets, address-block sizes, interrupt bindings and the reset of bits no header field states from it; every header disagreement is decided by the header.",
                    pin.repository, pin.revision, pin.path, pin.sha256
                ),
            );
        }
        let mut texts = BTreeMap::new();
        let mut read = |path: &str| -> Result<(usize, Pin), ImportError> {
            if let Some(found) = texts.get(path) {
                return Ok(Clone::clone(found));
            }
            let (text, pin) = pinned.idf(path)?;
            if pin.revision != self.revision {
                return Err(ImportError::new(
                    format!("{}:{path}", pin.source),
                    format!(
                        "is pinned at {}, the import's ESP-IDF at {}",
                        pin.revision, self.revision
                    ),
                ));
            }
            let found = (text.lines().count(), pin);
            texts.insert(path.to_owned(), found.clone());
            Ok(found)
        };
        for path in &self.files {
            let (_, pin) = read(path)?;
            entries.insert(
                self.idf_id(path),
                format!(
                    "ESP-IDF {} pinned in artifacts.toml, {}:{path}, sha256 {}, Apache-2.0: the register header whose definitions, field bit positions, access classes, defaults and descriptions the imported registers and fields take.",
                    pin.revision, pin.source, pin.sha256
                ),
            );
        }
        for (path, line) in &self.lines {
            let (count, pin) = read(path)?;
            if *line > count {
                return Err(ImportError::new(
                    format!("{}:{path}:{line}", pin.source),
                    format!("the pinned file has {count} lines"),
                ));
            }
            entries.insert(
                format!("{}_L{line}", self.idf_id(path)),
                format!(
                    "ESP-IDF {} pinned in artifacts.toml, {}:{path}:{line} (file sha256 {}), Apache-2.0: the line an imported fact cites.",
                    pin.revision, pin.source, pin.sha256
                ),
            );
        }
        for (id, description) in entries {
            let _ = write!(
                out,
                "\n[[sources]]\nid = {}\ndescription = {}\n",
                string(&id),
                string(&description)
            );
        }
        Ok(out)
    }
}

fn review_keys(
    review: &Review,
    default: &Cites,
    catalog: &mut Catalog,
    headers: &[String],
) -> Vec<String> {
    let mut keys = Vec::new();
    if review.sources != *default {
        keys.push(format!(
            "sources = {}",
            list(&catalog.ids(&review.sources, headers))
        ));
    }
    if review.derived {
        keys.push("provenance = \"derived\"".to_owned());
    }
    keys
}

/// One type file.
pub(super) fn type_file(out: &OutType, catalog: &mut Catalog) -> Result<String, ImportError> {
    let headers = &out.headers;
    let mut default = Cites::from([Cite::Header]);
    if out.svd {
        default.insert(Cite::Svd);
    }
    let default_ids = catalog.ids(&default, headers);
    let mut text = String::new();
    let _ = write!(
        text,
        "# Imported by `cargo registers import` from the pinned vendor SVD and\n\
         # ESP-IDF headers (registers/import.toml).\n\
         schema = 3\n\n[type]\nname = {}\ndescription = {}\nsources = {}\n",
        string(&out.spec.name),
        string(&out.spec.description),
        list(&default_ids)
    );
    let _ = write!(
        text,
        "\n[review]\nsources = {}\nprovenance = \"imported\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"vendor\"\n",
        list(&default_ids)
    );
    for register in &out.block.registers {
        let keys = review_keys(&register.review, &default, catalog, headers);
        if !keys.is_empty() {
            let _ = write!(
                text,
                "\n[review.entity.{}]\n{}\n",
                string(&register.name),
                keys.join("\n")
            );
        }
        for field in &register.fields {
            let keys = review_keys(&field.review, &default, catalog, headers);
            if !keys.is_empty() {
                let _ = write!(
                    text,
                    "\n[review.entity.{}]\n{}\n",
                    string(&format!("{}.{}", register.name, field.name)),
                    keys.join("\n")
                );
            }
        }
    }
    for register in &out.block.registers {
        register_text(&mut text, register, catalog, headers);
    }
    for command in &out.block.commands {
        let sources = catalog.ids(&command.sources, headers);
        let _ = write!(
            text,
            "\n[[commands]]\nregister = {}\ncommands = {}\n",
            string(&command.register),
            list(&command.commands)
        );
        if !command.qualifiers.is_empty() {
            let _ = writeln!(text, "qualifiers = {}", list(&command.qualifiers));
        }
        if !command.held.is_empty() {
            let _ = writeln!(text, "held = {}", list(&command.held));
        }
        let _ = writeln!(text, "sources = {}", list(&sources));
    }
    Ok(text)
}

fn register_text(
    text: &mut String,
    register: &OutRegister,
    catalog: &mut Catalog,
    headers: &[String],
) {
    let _ = write!(
        text,
        "\n[[registers]]\nname = {}\noffset = {:#x}\n",
        string(&register.name),
        register.offset
    );
    if register.width != 32 {
        let _ = writeln!(text, "width = {}", register.width);
    }
    if let Some((count, stride)) = register.dim {
        let _ = writeln!(text, "dim = {{ count = {count}, stride = {stride:#x} }}");
    }
    if let Some(description) = &register.description {
        let _ = writeln!(text, "description = {}", string(description));
    }
    if let Some((value, mask)) = register.reset {
        let full = if register.width >= 32 {
            u32::MAX
        } else {
            (1 << register.width) - 1
        };
        if mask == full {
            let _ = writeln!(text, "reset = {{ value = {value:#x} }}");
        } else {
            let _ = writeln!(text, "reset = {{ value = {value:#x}, mask = {mask:#x} }}");
        }
    }
    if register.reserved != 0 {
        let sources = catalog.ids(&Cites::from([Cite::Header]), headers);
        let _ = writeln!(
            text,
            "reserved = {{ mask = {:#x}, sources = {} }}",
            register.reserved,
            list(&sources)
        );
    }
    for (key, port) in [
        ("fifo", &register.fifo),
        ("data-port", &register.data_port),
        ("read-side-effect", &register.read_side_effect),
    ] {
        if let Some(cites) = port {
            let sources = catalog.ids(cites, headers);
            let _ = writeln!(text, "{key} = {{ sources = {} }}", list(&sources));
        }
    }
    for field in &register.fields {
        let _ = write!(
            text,
            "\n[[registers.fields]]\nname = {}\nbits = {}\nclass = {}\n",
            string(&field.name),
            string(&bits(field.low, field.width)),
            string(field.class.label())
        );
        if let Some(description) = &field.description {
            let _ = writeln!(text, "description = {}", string(description));
        }
        if !field.values.is_empty() {
            let values = field
                .values
                .iter()
                .map(|value| match &value.description {
                    Some(description) => format!(
                        "{{ name = {}, value = {}, description = {} }}",
                        string(&value.name),
                        value.value,
                        string(description)
                    ),
                    None => format!(
                        "{{ name = {}, value = {} }}",
                        string(&value.name),
                        value.value
                    ),
                })
                .collect::<Vec<_>>();
            let _ = writeln!(text, "values = [{}]", values.join(", "));
        }
    }
}

/// The chip root: the type includes, the instances and the interrupt
/// sources bound to them.
pub(super) fn root_file(
    spec: &ImportSpec,
    types: &BTreeMap<String, String>,
    instances: &[OutInstance],
    interrupts: &[OutInterrupt],
    catalog: &mut Catalog,
) -> Result<String, ImportError> {
    let mut text = String::new();
    let paths = types.keys().cloned().collect::<Vec<_>>();
    let _ = write!(
        text,
        "# The {} chip root of the imported register blocks, written by\n\
         # `cargo registers import` (registers/import.toml).\n\
         schema = 4\nchip = {}\ntypes = [\n",
        spec.chip,
        string(&spec.chip)
    );
    for path in &paths {
        let _ = writeln!(text, "    {},", string(path));
    }
    text.push_str("]\n");
    for instance in instances {
        let sources = catalog.ids(&instance.sources, &[]);
        let _ = write!(
            text,
            "\n[[instances]]\nname = {}\ntype = {}\nbase = {:#x}\nextent = {{ size = {:#x} }}\nsources = {}\n",
            string(&instance.name),
            string(&instance.block_type),
            instance.base,
            instance.size,
            list(&sources)
        );
    }
    for interrupt in interrupts {
        let sources = catalog.ids(&interrupt.sources, &[]);
        let _ = write!(
            text,
            "\n[[interrupts]]\nname = {}\nsource = {}\ninstance = {}\nsources = {}\n",
            string(&interrupt.name),
            interrupt.number,
            string(&interrupt.instance),
            list(&sources)
        );
    }
    Ok(text)
}
