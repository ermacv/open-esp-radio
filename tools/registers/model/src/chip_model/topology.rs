//! Resolve hart-sized families from the chip's platform list before validation.
use super::{Error, ErrorKind, RootFile, TypeFile};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub harts: u32,
    pub boot_hart: u32,
}

pub(super) fn count(base: &Path, file: Option<&str>) -> Result<Option<Geometry>, Error> {
    let Some(file) = file else { return Ok(None) };
    let path = base.join(file);
    #[derive(Deserialize)]
    struct Platform {
        interrupts: Interrupts,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Interrupts {
        harts: Vec<u32>,
        boot_hart: u32,
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| Error::new(&path, "harts", ErrorKind::Read(e)))?;
    let platform: Platform = toml_edit::de::from_str(&text)
        .map_err(|e| Error::new(&path, "harts", ErrorKind::Parse(e.to_string())))?;
    let n = u32::try_from(platform.interrupts.harts.len())
        .map_err(|e| Error::new(&path, "harts", ErrorKind::Parse(e.to_string())))?;
    if n == 0 || n > u32::from(u8::MAX) || !platform.interrupts.harts.iter().copied().eq(0..n) {
        return Err(Error::new(
            &path,
            "harts",
            ErrorKind::Dim(
                "harts must be the nonempty contiguous list 0..N, fitting a byte".into(),
            ),
        ));
    }
    if platform.interrupts.boot_hart >= n {
        return Err(Error::new(
            &path,
            "boot-hart",
            ErrorKind::Dim("boot hart lies outside the geometry".into()),
        ));
    }
    Ok(Some(Geometry {
        harts: n,
        boot_hart: platform.interrupts.boot_hart,
    }))
}

pub(super) fn resolve(
    text: &str,
    geometry: Option<Geometry>,
    path: &Path,
) -> Result<String, Error> {
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| Error::new(path, "", ErrorKind::Parse(e.to_string())))?;
    fn value(v: &mut toml_edit::Value, geometry: Option<Geometry>) -> Result<(), &'static str> {
        match v {
            toml_edit::Value::InlineTable(t) => {
                for (key, v) in t.iter_mut() {
                    if key == "count"
                        && let Some(symbol) = v.as_str()
                    {
                        let n = geometry.ok_or("a symbolic count needs hart-facts")?.harts;
                        let count = match symbol {
                            "harts" => n,
                            "secondary-harts" => n - 1,
                            _ => return Err("unknown count symbol"),
                        };
                        *v = toml_edit::Value::from(i64::from(count));
                    } else if key == "first"
                        && let Some(symbol) = v.as_str()
                    {
                        let geometry = geometry.ok_or("a symbolic first label needs hart-facts")?;
                        if symbol != "first-secondary-hart" {
                            return Err("unknown first label symbol");
                        }
                        let mut secondaries =
                            (0..geometry.harts).filter(|h| *h != geometry.boot_hart);
                        let first = secondaries
                            .next()
                            .ok_or("the geometry has no secondary hart")?;
                        if !secondaries.eq(first + 1..first + geometry.harts - 1) {
                            return Err("a secondary register family needs contiguous hart labels");
                        }
                        *v = toml_edit::Value::from(i64::from(first));
                    } else {
                        value(v, geometry)?;
                    }
                }
            }
            toml_edit::Value::Array(a) => {
                for v in a.iter_mut() {
                    value(v, geometry)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    fn item(i: &mut toml_edit::Item, geometry: Option<Geometry>) -> Result<(), &'static str> {
        match i {
            toml_edit::Item::Value(v) => value(v, geometry)?,
            toml_edit::Item::Table(t) => {
                for (_, i) in t.iter_mut() {
                    item(i, geometry)?;
                }
            }
            toml_edit::Item::ArrayOfTables(a) => {
                for t in a.iter_mut() {
                    for (_, i) in t.iter_mut() {
                        item(i, geometry)?;
                    }
                }
            }
            _ => (),
        }
        Ok(())
    }
    item(doc.as_item_mut(), geometry)
        .map_err(|e| Error::new(path, "", ErrorKind::Dim(e.into())))?;
    Ok(doc.to_string())
}

pub(super) fn instances(root: &mut RootFile, path: &Path) -> Result<(), Error> {
    let mut expanded = Vec::new();
    for instance in std::mem::take(&mut root.instances) {
        let Some(dim) = instance.dim else {
            expanded.push(instance);
            continue;
        };
        if instance.hart_local.is_some() {
            return Err(Error::new(
                path,
                &instance.name,
                ErrorKind::HartLocal(
                    "an instance family has one address per element, not one per hart".into(),
                ),
            ));
        }
        super::block_type::check_name(&instance.name, true)
            .map_err(|e| Error::new(path, &instance.name, e))?;
        if dim.count == 0
            || dim.count > u32::from(u8::MAX)
            || dim.first.checked_add(dim.count - 1).is_none()
            || dim.stride == 0
        {
            return Err(Error::new(
                path,
                &instance.name,
                ErrorKind::Dim("an instance family has a positive count and stride".into()),
            ));
        }
        for index in 0..dim.count {
            let mut element = instance.clone();
            element.name = instance
                .name
                .replace("%s", &dim.first.wrapping_add(index).to_string());
            element.base = instance
                .base
                .checked_add(u64::from(index) * u64::from(dim.stride))
                .ok_or_else(|| {
                    Error::new(
                        path,
                        &instance.name,
                        ErrorKind::Dim("instance address overflow".into()),
                    )
                })?;
            element.dim = None;
            expanded.push(element);
        }
    }
    root.instances = expanded;
    Ok(())
}

pub(super) fn fields(file: &mut TypeFile, path: &Path) -> Result<(), Error> {
    if file
        .groups
        .iter()
        .any(|g| g.registers.iter().any(|r| !r.element_resets.is_empty()))
    {
        return Err(Error::new(
            path,
            "element-resets",
            ErrorKind::Dim("per-element resets belong to top-level register families".into()),
        ));
    }
    for register in file
        .registers
        .iter_mut()
        .chain(file.groups.iter_mut().flat_map(|g| g.registers.iter_mut()))
    {
        let mut expanded = Vec::new();
        for field in std::mem::take(&mut register.fields) {
            let Some(dim) = field.dim else {
                expanded.push(field);
                continue;
            };
            super::block_type::check_name(&field.name, true)
                .map_err(|e| Error::new(path, &field.name, e))?;
            if !matches!(
                field.class,
                super::source::FieldClass::Plain | super::source::FieldClass::ReadOnly
            ) || !field.values.is_empty()
                || field.pointer.is_some()
                || field.write_range.is_some()
                || field.value_range.is_some()
            {
                return Err(Error::new(
                    path,
                    &field.name,
                    ErrorKind::Dim("field families admit plain or read-only native lanes".into()),
                ));
            }
            let (low, width) = super::block_type::parse_bits(&field.bits)
                .map_err(|e| Error::new(path, &field.name, e))?;
            if dim.count == 0 || dim.count > 32 || dim.stride == 0 {
                return Err(Error::new(
                    path,
                    &field.name,
                    ErrorKind::Dim("a field family has 1..=32 nonoverlapping elements".into()),
                ));
            }
            for index in 0..dim.count {
                let low = i64::from(low) + i64::from(index) * i64::from(dim.stride);
                if low < 0 || low + i64::from(width) > 32 {
                    return Err(Error::new(
                        path,
                        &field.name,
                        ErrorKind::Dim("field family lies outside its word".into()),
                    ));
                }
                let mut element = field.clone();
                element.name = field.name.replace("%s", &index.to_string());
                element.bits = format!("{}:{low}", low + i64::from(width) - 1);
                element.dim = None;
                element.family = Some(super::source::FieldFamily {
                    name: field.name.clone(),
                    index,
                    count: dim.count,
                    stride: dim.stride,
                });
                expanded.push(element);
            }
        }
        register.fields = expanded;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_hart_is_required_and_uses_the_platform_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("platform.toml");
        std::fs::write(&path, "[interrupts]\nharts = [0, 1, 2]\n").unwrap();
        let error = count(directory.path(), Some("platform.toml")).unwrap_err();
        assert!(error.to_string().contains("boot-hart"), "{error}");
        std::fs::write(&path, "[interrupts]\nharts = [0, 1, 2]\nboot-hart = 2\n").unwrap();
        let geometry = count(directory.path(), Some("platform.toml"))
            .unwrap()
            .unwrap();
        assert_eq!(geometry.boot_hart, 2);
    }

    #[test]
    fn secondary_labels_follow_the_explicit_boot_role() {
        for (boot_hart, first) in [(0, 1), (2, 0)] {
            let text = resolve(
                "dim = { count = 'secondary-harts', first = 'first-secondary-hart', stride = 4 }",
                Some(Geometry {
                    harts: 3,
                    boot_hart,
                }),
                Path::new("synthetic.toml"),
            )
            .unwrap();
            let value: toml_edit::DocumentMut = text.parse().unwrap();
            assert_eq!(value["dim"]["first"].as_integer(), Some(first));
            assert_eq!(value["dim"]["count"].as_integer(), Some(2));
        }
        for geometry in [
            Geometry {
                harts: 1,
                boot_hart: 0,
            },
            Geometry {
                harts: 3,
                boot_hart: 1,
            },
        ] {
            assert!(resolve(
                "dim = { count = 'secondary-harts', first = 'first-secondary-hart', stride = 4 }",
                Some(geometry),
                Path::new("synthetic.toml"),
            ).is_err(), "an empty or noncontiguous secondary label range is refused");
        }
    }
}
