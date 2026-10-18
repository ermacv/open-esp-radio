use std::path::Path;

use crate::Result;
use oer_process::Checkout;

pub use oer_repo::{Classification, Scope};

/// The Rust target of the chip `id`, from its profile.
pub fn rust_target(root: &Path, id: &str) -> Result<String> {
    oer_repo::chips::Chips::at(root)?
        .profile(id)
        .map(|profile| profile.rust_target.clone())
        .ok_or_else(|| format!("no chip profile `{id}`").into())
}

/// One classified package of the repository model.
#[derive(Clone, Debug)]
pub struct Classified {
    pub package: oer_repo::Package,
    pub class: Classification,
}

pub fn model(ctx: &Checkout) -> Result<oer_repo::Model> {
    Ok(oer_repo::Model::load(&oer_repo::Repo::from_git(
        &ctx.root,
    )?)?)
}

/// Every package of every workspace with its classification; a package the
/// model cannot classify fails, so none disappears.
pub fn source_packages(model: &oer_repo::Model) -> Result<Vec<Classified>> {
    let mut packages = Vec::new();
    for package in model.packages() {
        let class = model
            .classification(package)
            .map_err(|error| format!("{}: {error}", package.manifest))?;
        packages.push(Classified {
            package: package.clone(),
            class: class.clone(),
        });
    }
    if packages.is_empty() {
        return Err("no classified source packages found".into());
    }
    Ok(packages)
}

/// The production packages of every workspace.
pub fn production_packages(model: &oer_repo::Model) -> Result<Vec<Classified>> {
    let packages: Vec<Classified> = source_packages(model)?
        .into_iter()
        .filter(|package| package.class.scope == Scope::Production)
        .collect();
    if packages.is_empty() {
        return Err("no production packages found".into());
    }
    Ok(packages)
}

#[cfg(test)]
mod tests;
