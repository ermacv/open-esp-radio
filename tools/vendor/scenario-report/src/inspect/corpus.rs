//! Verified inputs and structural names for presenting Blobray's records.
use super::*;
use app::in_process::Executable;
use oer_analysis_elf::{SymbolKind, classify};
use oer_analysis_riscv_model::{ObjectId, SymbolId};
use std::path::Path;

pub struct Corpus {
    pub(super) inputs: Vec<Executable>,
    roles: Vec<String>,
    pub(super) registers: Registers,
    pub missing: Vec<String>,
}

impl Corpus {
    pub fn load(root: &Path) -> Result<Self> {
        let mut corpus = Self {
            inputs: vec![],
            roles: vec![],
            missing: vec![],
            registers: Registers::load(root, oer_vendor_scenario::chip().registers)?,
        };
        for artifact in &oer_vendor_scenario::artifacts::manifest().artifact {
            if ![".a", ".elf"]
                .iter()
                .any(|extension| artifact.path.ends_with(extension))
            {
                continue;
            }
            let path = oer_vendor_scenario::artifacts::path(root, &artifact.id);
            let Ok(bytes) = std::fs::read(&path) else {
                corpus.missing.push(artifact.id.clone());
                continue;
            };
            let Ok(pin) = oer_vendor_scenario::artifacts::sha256(&artifact.id) else {
                corpus.missing.push(artifact.id.clone());
                continue;
            };
            let executable = Executable::new(bytes);
            if executable.id().as_str() != pin {
                return Err(invalid(format!(
                    "{} at {} has sha256 {}, not the pinned {pin}",
                    artifact.id,
                    path.display(),
                    executable.id()
                )));
            }
            corpus.inputs.push(executable);
            corpus.roles.push(artifact.id.clone());
        }
        Ok(corpus)
    }

    pub fn of(objects: &[(&str, &[u8])], registers: Registers) -> Result<Self> {
        Ok(Self {
            inputs: objects
                .iter()
                .map(|(_, bytes)| Executable::new(bytes.to_vec()))
                .collect(),
            roles: objects.iter().map(|(role, _)| (*role).to_owned()).collect(),
            registers,
            missing: vec![],
        })
    }

    pub(super) fn metadata(
        &self,
        strings_for: Option<&[String]>,
        budget: &mut Budget,
    ) -> Result<Metadata> {
        let mut out = Metadata::default();
        for (input, executable) in self.inputs.iter().enumerate() {
            let inventory =
                app::captured::inventory(executable, &budget.memory, &mut budget.control)?;
            let role = self
                .roles
                .get(input)
                .ok_or_else(|| invalid("input has no artifact name"))?;
            for object in &inventory.objects {
                let Some(elf) = &object.elf else { continue };
                let origin = match &object.name {
                    Some(member) => format!("{role}[{}]", String::from_utf8_lossy(member)),
                    None => role.clone(),
                };
                for symbol in &elf.symbols {
                    out.symbols.insert(symbol.id.clone(), symbol.clone());
                    if SymbolKind::of_type(symbol.symbol_type) != SymbolKind::Text
                        || !classify::symbol_section(symbol.raw_section, symbol.extended_section)
                            .is_defined()
                    {
                        continue;
                    }
                    let name = String::from_utf8_lossy(symbol.name.as_deref().unwrap_or_default())
                        .into_owned();
                    out.functions.insert(
                        (input as u64, symbol.id.clone()),
                        Function {
                            origin: origin.clone(),
                            name: name.clone(),
                            entry: symbol.value,
                            size: symbol.size,
                        },
                    );
                    if classify::object_kind(elf.object_type) == classify::ObjectKind::Executable {
                        out.names.entry(symbol.value).or_insert(name);
                    }
                }
                let wanted = strings_for.is_some_and(|names| {
                    names.is_empty()
                        || elf.symbols.iter().any(|symbol| {
                            symbol.name.as_deref().is_some_and(|name| {
                                names.iter().any(|wanted| wanted.as_bytes() == name)
                            })
                        })
                });
                if wanted {
                    for section in app::symbols::read_only_data(elf) {
                        let bytes = app::symbols::section_bytes(
                            &object.id,
                            section,
                            &self.inputs,
                            &budget.memory,
                            &mut budget.control,
                        )?;
                        out.sections
                            .insert((object.id.clone(), section.index), bytes);
                        out.section_addresses
                            .insert((object.id.clone(), section.index), section.address);
                    }
                }
            }
        }
        Ok(out)
    }
}

#[derive(Default)]
pub(super) struct Metadata {
    pub functions: BTreeMap<(u64, SymbolId), Function>,
    pub symbols: BTreeMap<SymbolId, oer_analysis_blobray_domain::SymbolRecord>,
    pub sections: BTreeMap<(ObjectId, u32), Vec<u8>>,
    pub section_addresses: BTreeMap<(ObjectId, u32), u64>,
    pub names: BTreeMap<u64, String>,
}

pub(super) struct Function {
    pub origin: String,
    pub name: String,
    pub entry: u64,
    pub size: u64,
}

impl Metadata {
    pub fn function(&self, function: &LibraryFunction) -> Result<&Function> {
        self.functions
            .get(&(function.input, function.symbol.clone()))
            .ok_or_else(|| invalid("analysis function has no captured symbol"))
    }

    pub fn literal(&self, symbol: &SymbolId, addend: i64) -> Option<String> {
        let info = self.symbols.get(symbol)?;
        let section = classify::symbol_section(info.raw_section, info.extended_section).index()?;
        let bytes = self.sections.get(&(symbol.object.clone(), section))?;
        let base = self
            .section_addresses
            .get(&(symbol.object.clone(), section))?;
        app::symbols::string_at(
            bytes,
            info.value.checked_add_signed(addend)?.checked_sub(*base)?,
        )
        .filter(|text| !text.is_empty())
    }

    pub fn literal_value(
        &self,
        object: &ObjectId,
        value: &oer_analysis_riscv_model::AbstractValue,
    ) -> Option<String> {
        use oer_analysis_riscv_model::AbstractValue;
        match value {
            AbstractValue::Symbol { symbol, addend } => self.literal(symbol, *addend),
            AbstractValue::Section { section, offset } => app::symbols::string_at(
                self.sections.get(&(object.clone(), *section))?,
                u64::try_from(*offset).ok()?,
            ),
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                self.sections
                    .iter()
                    .filter(|((id, _), _)| id == object)
                    .find_map(|(key, bytes)| {
                        let offset =
                            u64::from(*value).checked_sub(*self.section_addresses.get(key)?)?;
                        app::symbols::string_at(bytes, offset)
                    })
            }
            _ => None,
        }
        .filter(|text| !text.is_empty())
    }
}
