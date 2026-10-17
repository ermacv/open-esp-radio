//! Native unreviewed source initialization; no binary observations are declarations.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PeripheralInit {
    pub name: String,
    pub base: u64,
    pub length: u32,
}

/// Explicit geometry, not an inferred register/access catalogue.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ModelInit {
    pub schema: u32,
    pub chip: String,
    pub address_space: String,
    pub device: ModelDevice,
    pub peripherals: Vec<PeripheralInit>,
}

/// Validated native source text. The host owns exclusive creation and publication.
pub struct ModelDraft {
    pub manifest: String,
    pub fragment: String,
    pub peripherals: usize,
}
impl ModelDraft {
    /// Initialize empty peripherals; no register width, fields or access semantics
    /// are inferred. Device defaults are explicitly supplied by the caller.
    pub fn initialize(request: ModelInit) -> Result<Self> {
        if request.schema != 1 || request.peripherals.is_empty() {
            return Err(Error::message(
                "model initialization requires schema 1 and peripherals",
            ));
        }
        let mut peripherals = Vec::new();
        for (i, p) in request.peripherals.iter().enumerate() {
            let end = p
                .base
                .checked_add(u64::from(p.length))
                .filter(|_| p.length != 0)
                .ok_or_else(|| Error::message("invalid peripheral range"))?;
            if request.peripherals[..i]
                .iter()
                .any(|q| p.base < q.base.saturating_add(u64::from(q.length)) && q.base < end)
            {
                return Err(Error::message("initial peripheral ranges overlap"));
            }
            let block = svd_rs::AddressBlock::builder()
                .offset(0)
                .size(p.length)
                .usage(AddressBlockUsage::Registers)
                .build(ValidateLevel::Strict)?;
            peripherals.push(MaybeArray::Single(
                svd_rs::PeripheralInfo::builder()
                    .name(p.name.clone())
                    .base_address(p.base)
                    .address_block(Some(vec![block]))
                    .build(ValidateLevel::Strict)?,
            ));
        }
        Self::native(
            request.chip,
            request.address_space,
            request.device,
            peripherals,
        )
    }

    /// Import standard CMSIS-SVD declarations without granting reviewed status.
    /// The host must retain `xml` verbatim, including extensions outside this model.
    pub fn import_svd(xml: &str, chip: String, address_space: String) -> Result<Self> {
        let d = svd_parser::parse(xml).map_err(|e| Error::message(e.to_string()))?;
        let metadata = ModelDevice {
            name: d.name,
            version: d.version,
            description: d.description,
            vendor: d.vendor,
            vendor_id: d.vendor_id,
            series: d.series,
            license_text: d.license_text,
            cpu: d.cpu,
            header_system_filename: d.header_system_filename,
            header_definitions_prefix: d.header_definitions_prefix,
            address_unit_bits: d.address_unit_bits,
            width: d.width,
            register_defaults: d.default_register_properties,
            svd_schema: d.schema_version,
            svd_schema_location: d.no_namespace_schema_location,
        };
        Self::native(chip, address_space, metadata, d.peripherals)
    }

    fn native(
        chip: String,
        address_space: String,
        device: ModelDevice,
        peripherals: Vec<Peripheral>,
    ) -> Result<Self> {
        SemanticEntityId::register(&chip, &address_space, 0, 1)
            .map_err(|e| Error::message(e.to_string()))?;
        if peripherals.is_empty() {
            return Err(Error::message("model requires peripherals"));
        }
        validate_peripheral_names(&peripherals)?;
        let built = build_device(&device, peripherals.clone())?;
        model_validation::validate_device(&built)?;
        // Exercise the same physical identity checks as a reopened source model.
        let model = RegisterModel {
            loaded_inputs: BTreeMap::new(),
            chip: chip.clone(),
            address_space: address_space.clone(),
            device: built,
            review: vec![],
            reviewed_register_facts: vec![],
            shared_peripherals: BTreeMap::new(),
        };
        model.register_identities()?;
        Ok(Self {
            peripherals: peripherals.len(),
            manifest: toml_edit::ser::to_string_pretty(&RegisterModelManifest {
                schema: 3,
                chip,
                address_space,
                device,
                fragments: vec!["peripherals.toml".into()],
                shared_fragments: vec![],
            })?,
            fragment: toml_edit::ser::to_string_pretty(&RegisterModelFragment {
                schema: 2,
                peripherals,
                review: vec![],
            })?,
        })
    }
}
