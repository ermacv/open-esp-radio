//! No two generated items share a name in one Rust namespace: the types
//! and the values of each module, and the associated items of each type's
//! inherent impls together. The renderer's own name rules make most
//! collisions model errors; this check refuses any left.

use std::collections::{BTreeMap, BTreeSet};

use syn::{Fields, ImplItem, Item, Type, UseTree};

use super::Error;

/// Refuse `rust` when two of its items collide.
pub(super) fn check(rust: &str) -> Result<(), Error> {
    let file = syn::parse_file(rust)
        .map_err(|error| Error::new("rendered Rust", format!("does not parse: {error}")))?;
    module("crate", &file.items)
}

fn module(path: &str, items: &[Item]) -> Result<(), Error> {
    let mut types = BTreeSet::new();
    let mut values = BTreeSet::new();
    let mut associated: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let insert = |set: &mut BTreeSet<String>, name: String| {
        if name == "_" || set.insert(name.clone()) {
            Ok(())
        } else {
            Err(Error::new(
                &format!("{path}::{name}"),
                "is generated twice in one namespace",
            ))
        }
    };
    for item in items {
        match item {
            Item::Struct(item) => {
                insert(&mut types, item.ident.to_string())?;
                if !matches!(item.fields, Fields::Named(_)) {
                    insert(&mut values, item.ident.to_string())?;
                }
            }
            Item::Enum(item) => insert(&mut types, item.ident.to_string())?,
            Item::Type(item) => insert(&mut types, item.ident.to_string())?,
            Item::Trait(item) => insert(&mut types, item.ident.to_string())?,
            Item::Mod(item) => {
                insert(&mut types, item.ident.to_string())?;
                if let Some((_, content)) = &item.content {
                    module(&format!("{path}::{}", item.ident), content)?;
                }
            }
            Item::Const(item) => insert(&mut values, item.ident.to_string())?,
            Item::Static(item) => insert(&mut values, item.ident.to_string())?,
            Item::Fn(item) => insert(&mut values, item.sig.ident.to_string())?,
            Item::Use(item) => {
                let mut leaves = Vec::new();
                use_leaves(&item.tree, &mut leaves);
                for leaf in leaves {
                    insert(&mut types, leaf)?;
                }
            }
            Item::Impl(item) if item.trait_.is_none() => {
                let Type::Path(self_type) = &*item.self_ty else {
                    continue;
                };
                let Some(name) = self_type.path.segments.last() else {
                    continue;
                };
                let set = associated.entry(name.ident.to_string()).or_default();
                for member in &item.items {
                    let ident = match member {
                        ImplItem::Fn(member) => &member.sig.ident,
                        ImplItem::Const(member) => &member.ident,
                        _ => continue,
                    };
                    if !set.insert(ident.to_string()) {
                        return Err(Error::new(
                            &format!("{path}::{}::{ident}", name.ident),
                            "is generated twice for one type",
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn use_leaves(tree: &UseTree, leaves: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => use_leaves(&path.tree, leaves),
        UseTree::Name(name) if name.ident != "self" => leaves.push(name.ident.to_string()),
        UseTree::Rename(rename) => leaves.push(rename.rename.to_string()),
        UseTree::Group(group) => {
            for tree in &group.items {
                use_leaves(tree, leaves);
            }
        }
        _ => {}
    }
}
