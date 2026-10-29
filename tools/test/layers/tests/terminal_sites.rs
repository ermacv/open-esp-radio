//! Source inventory, not a claim about machine-code reachability or hardware proofs.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    reason = "integration-test helpers report malformed source and broken fixture invariants"
)]

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use proc_macro2::{Span, TokenStream, TokenTree};
use syn::{Attribute, ReturnType, Type, spanned::Spanned, visit::Visit};

const CLASSES: &[&str] = &[
    "hardware-impossible",
    "unexpected-hardware",
    "software-invariant",
    "configuration-boot",
    "exhaustion",
    "cancellation",
    "cpu-exception",
    "requested-reset",
    "rust-panic",
];

// These enums supply retained diagnostics. Ordinary driver errors are not a
// terminal decision; the registry names the image/platform owner of each reason.
const REASON_ENUMS: &[(&str, &str)] = &[
    (
        "chips/espressif/staged-runtime/src/panic/fault.rs",
        "FaultReason",
    ),
    (
        "chips/esp32s31/hal/src/domain/transaction.rs",
        "FaultReason",
    ),
    ("firmware/hil/esp32s31/src/fatal.rs", "Failure"),
    (
        "chips/espressif/ieee802154-engine/src/engine/fault.rs",
        "Fault",
    ),
    (
        "chips/espressif/ieee802154-engine/src/engine/fault.rs",
        "ServiceError",
    ),
];

// Numeric codecs without a reason enum: every numeric arm in their next-boot
// name decoder is inventoried, including payload/operation subcodes.
const REASON_DECODERS: &[(&str, &str)] = &[(
    "chips/esp32s31/hal/src/domain/transaction/record.rs",
    "report_fault",
)];

type Sites = BTreeSet<String>;
type ScannedSites = BTreeMap<String, Location>;

#[derive(Debug, PartialEq, Eq)]
struct Location {
    line: usize,
    column: usize,
}

fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr.parse_args::<syn::Meta>().is_ok_and(|meta| match meta {
                    syn::Meta::Path(path) => path.is_ident("test"),
                    syn::Meta::List(list) if list.path.is_ident("all") => list
                        .parse_args_with(
                            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                        )
                        .is_ok_and(|items| items.iter().any(|item| matches!(item, syn::Meta::Path(path) if path.is_ident("test")))),
                    _ => false,
                }))
    })
}

fn never(output: &ReturnType) -> bool {
    matches!(output, ReturnType::Type(_, ty) if matches!(**ty, Type::Never(_)))
}

fn sources(directory: &Path, files: &mut Vec<PathBuf>) {
    if !directory.exists() {
        return;
    }
    for entry in std::fs::read_dir(directory).expect("read source directory") {
        let entry = entry.expect("source entry");
        let path = entry.path();
        if path.is_dir() {
            if !["target", ".git", "tests"].contains(&entry.file_name().to_string_lossy().as_ref())
            {
                sources(&path, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path
                .components()
                .any(|component| component.as_os_str() == "src")
            && !path.ends_with("pac/src/generated.rs")
            && path.file_name().is_none_or(|name| name != "tests.rs")
        {
            files.push(path);
        }
    }
}

struct Scanner<'a> {
    file: &'a str,
    source: &'a str,
    item: Vec<String>,
    ordinals: BTreeMap<String, usize>,
    sites: ScannedSites,
    decoder: bool,
    aliases: BTreeMap<String, String>,
}

impl Scanner<'_> {
    fn add(&mut self, span: Span, kind: &str, name: impl std::fmt::Display) {
        let start = span.start();
        let prefix = format!("{}:{}:{kind}:{name}", self.file, self.item.join("::"));
        let ordinal = self.ordinals.entry(prefix.clone()).or_default();
        *ordinal += 1;
        self.sites.insert(
            format!("{prefix}:{ordinal}"),
            Location {
                line: start.line,
                column: start.column + 1,
            },
        );
    }

    fn tokens(&self, span: Span) -> String {
        self.source
            .get(span.byte_range())
            .expect("AST span is inside its source")
            .parse::<TokenStream>()
            .expect("AST item tokens")
            .to_string()
    }

    fn image_panic_scope(&self) -> bool {
        self.file.starts_with("firmware/")
            || self.file.starts_with("chips/espressif/staged-runtime/")
            || self.file.starts_with("chips/espressif/staged-bootstrap/")
    }

    // Macro bodies are token trees: inspect definitions as well as invocations,
    // so entry!/panic_handler! implementations and emitted panic paths count.
    fn macro_tokens(&mut self, tokens: TokenStream) {
        let tokens: Vec<_> = tokens.into_iter().collect();
        for (index, token) in tokens.iter().enumerate() {
            if let TokenTree::Ident(name) = token {
                let name_text = name.to_string();
                let resolved = self
                    .aliases
                    .get(&name_text)
                    .map_or(name_text.as_str(), String::as_str);
                let parent = index
                    .checked_sub(3)
                    .and_then(|index| tokens.get(index))
                    .and_then(|token| {
                        if let TokenTree::Ident(name) = token {
                            let name = name.to_string();
                            Some(self.aliases.get(&name).cloned().unwrap_or(name))
                        } else {
                            None
                        }
                    });
                if name_text == "__exception_handler" {
                    self.add(name.span(), "terminal-binding", name);
                } else if name_text == "panic_handler" {
                    self.add(name.span(), "panic-handler", name);
                } else if let Some(TokenTree::Punct(bang)) = tokens.get(index + 1)
                    && bang.as_char() == '!'
                    && self.image_panic_scope()
                    && panic_macro(resolved)
                {
                    self.add(name.span(), "panic-macro", name);
                } else if terminal_api(resolved)
                    || (name_text == "reset" && parent.as_deref().is_some_and(reset_module))
                    || (name_text == "handle"
                        && parent
                            .as_deref()
                            .is_some_and(|name| ["panic", "trap"].contains(&name)))
                {
                    self.add(name.span(), "terminal-reference", name);
                }
            } else if let TokenTree::Group(group) = token
                && !matches!(index.checked_sub(1).and_then(|previous| tokens.get(previous)), Some(TokenTree::Ident(name)) if name == "const")
            {
                self.macro_tokens(group.stream());
            }
        }
    }
}

fn panic_macro(name: &str) -> bool {
    [
        "panic",
        "unreachable",
        "todo",
        "unimplemented",
        "assert",
        "assert_eq",
        "assert_ne",
        "debug_assert",
        "debug_assert_eq",
        "debug_assert_ne",
        "vendor_assert",
    ]
    .contains(&name)
}

fn terminal_api(name: &str) -> bool {
    [
        "reset_recording",
        "reset_claimed",
        "reset_platform_once",
        "boot_failure",
        "reset_recording_with",
        "reset_fault",
        "reset_request",
        "reset_binding",
        "fatal_halt",
        "software_reset",
        "abort",
        "spin_forever",
        "__oer_phy_transaction_fault",
    ]
    .contains(&name)
}

fn terminal_path(path: &syn::Path, aliases: &BTreeMap<String, String>) -> bool {
    let names: Vec<_> = path
        .segments
        .iter()
        .map(|segment| {
            let name = segment.ident.to_string();
            aliases.get(&name).cloned().unwrap_or(name)
        })
        .collect();
    names.last().is_some_and(|name| {
        (terminal_api(name) && (name != "abort" || names.len() > 1))
            || (name == "reset" && names.iter().any(|part| reset_module(part)))
            || (name == "handle"
                && names
                    .iter()
                    .any(|part| ["panic", "trap"].contains(&part.as_str())))
    })
}

fn reset_module(name: &str) -> bool {
    ["fatal", "runtime", "staged", "oer_espressif_staged_runtime"].contains(&name)
        || name.ends_with("_fault")
}

fn aliases(tree: &syn::UseTree, prefix: Vec<String>, found: &mut BTreeMap<String, String>) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut prefix = prefix;
            prefix.push(path.ident.to_string());
            aliases(&path.tree, prefix, found);
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                aliases(tree, prefix.clone(), found);
            }
        }
        syn::UseTree::Name(name) => {
            let name = name.ident.to_string();
            if terminal_api(&name)
                || panic_macro(&name)
                || (name == "reset" && prefix.iter().any(|part| reset_module(part)))
            {
                found.insert(
                    name.clone(),
                    if name == "reset" {
                        "reset_recording".into()
                    } else {
                        name
                    },
                );
            }
        }
        syn::UseTree::Rename(rename) => {
            let name = rename.ident.to_string();
            let module = if name == "self" {
                prefix.last().map(String::as_str).unwrap_or("self")
            } else {
                &name
            };
            if ["fatal", "oer_espressif_staged_runtime"].contains(&module) {
                found.insert(rename.rename.to_string(), module.into());
            }
            if terminal_api(&name)
                || panic_macro(&name)
                || (name == "reset" && prefix.iter().any(|part| reset_module(part)))
            {
                found.insert(
                    rename.rename.to_string(),
                    if name == "reset" {
                        "reset_recording".into()
                    } else {
                        name
                    },
                );
            }
        }
        syn::UseTree::Glob(_) => {}
    }
}

#[derive(Default)]
struct Imports(BTreeMap<String, String>);

impl<'ast> Visit<'ast> for Imports {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !test_only(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }
    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if !test_only(&item.attrs) {
            syn::visit::visit_impl_item_fn(self, item);
        }
    }
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if !test_only(&item.attrs) {
            aliases(&item.tree, Vec::new(), &mut self.0);
        }
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let (attrs, name) = match item {
            syn::Item::Fn(item) => (&item.attrs, Some(item.sig.ident.to_string())),
            syn::Item::Mod(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Impl(item) => {
                let ty = self.tokens(item.self_ty.span());
                let name = if let Some((_, path, _)) = &item.trait_ {
                    format!("impl {} for {ty}", self.tokens(path.span()))
                } else {
                    format!("impl {ty}")
                };
                (&item.attrs, Some(name))
            }
            syn::Item::Macro(item) => (&item.attrs, item.ident.as_ref().map(ToString::to_string)),
            syn::Item::Enum(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Const(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Static(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Trait(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Struct(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Union(item) => (&item.attrs, Some(item.ident.to_string())),
            syn::Item::Type(item) => (&item.attrs, Some(item.ident.to_string())),
            _ => return syn::visit::visit_item(self, item),
        };
        if test_only(attrs) {
            return;
        }
        let named = name.is_some();
        if let Some(name) = name {
            self.item.push(name);
        }
        match item {
            syn::Item::Const(item) => {
                if (self.file == "chips/espressif/staged-runtime/src/panic.rs"
                    && item.ident == "INVALID_HART")
                    || (self.file == "chips/espressif/staged-runtime/src/trap.rs" && item.ident == "CPU_EXCEPTION")
                    || (self.file == "chips/espressif/staged-runtime/src/fatal.rs" && item.ident == "BOOT_FAILURE")
                    || (self.file == "firmware/hil/esp32s31/src/fatal.rs"
                        && ["PANIC", "EXCEPTION", "FAILURE"]
                            .iter()
                            .any(|name| item.ident == name))
                {
                    self.add(item.ident.span(), "retained-reason", &item.ident);
                }
            }
            syn::Item::Static(item) => {
                if item.attrs.iter().any(|attr| matches!(&attr.meta, syn::Meta::List(list) if list.path.is_ident("unsafe") && list.tokens.to_string().contains("noinit"))) {
                    self.add(item.ident.span(), "retained-slot", &item.ident);
                }
            }
            _ => syn::visit::visit_item(self, item),
        }
        if named {
            self.item.pop();
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if item
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("panic_handler"))
        {
            self.add(item.sig.ident.span(), "panic-handler", &item.sig.ident);
        }
        if never(&item.sig.output)
            && item.sig.asyncness.is_none()
            && !self.file.starts_with("portable/")
        {
            self.add(item.sig.ident.span(), "terminal-entry", &item.sig.ident);
        }
        let decoder = self.decoder;
        self.decoder = REASON_DECODERS
            .iter()
            .any(|&(file, name)| self.file == file && item.sig.ident == name);
        syn::visit::visit_item_fn(self, item);
        self.decoder = decoder;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if test_only(&item.attrs) {
            return;
        }
        self.item.push(item.sig.ident.to_string());
        if never(&item.sig.output)
            && item.sig.asyncness.is_none()
            && !self.file.starts_with("portable/")
        {
            self.add(item.sig.ident.span(), "terminal-entry", &item.sig.ident);
        }
        let decoder = self.decoder;
        self.decoder = REASON_DECODERS
            .iter()
            .any(|&(file, name)| self.file == file && item.sig.ident == name);
        syn::visit::visit_impl_item_fn(self, item);
        self.decoder = decoder;
        self.item.pop();
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        if !test_only(&item.attrs) {
            self.item.push(item.sig.ident.to_string());
            syn::visit::visit_trait_item_fn(self, item);
            self.item.pop();
        }
    }

    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if !test_only(&item.attrs) {
            self.item.push(item.ident.to_string());
            syn::visit::visit_impl_item_const(self, item);
            self.item.pop();
        }
    }

    fn visit_trait_item_const(&mut self, item: &'ast syn::TraitItemConst) {
        if !test_only(&item.attrs) {
            self.item.push(item.ident.to_string());
            syn::visit::visit_trait_item_const(self, item);
            self.item.pop();
        }
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if REASON_ENUMS
            .iter()
            .any(|&(file, name)| self.file == file && item.ident == name)
        {
            for variant in &item.variants {
                self.add(
                    variant.ident.span(),
                    "retained-reason",
                    format!("{}::{}", item.ident, variant.ident),
                );
            }
        }
        syn::visit::visit_item_enum(self, item);
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if self.decoder && numeric_pattern(&arm.pat) {
            self.add(arm.pat.span(), "retained-code", pattern_name(&arm.pat));
        }
        syn::visit::visit_arm(self, arm);
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if let syn::Expr::Const(_) = expr {
            return;
        }
        syn::visit::visit_expr(self, expr);
    }

    fn visit_expr_path(&mut self, expr: &'ast syn::ExprPath) {
        if let Some(segment) = expr.path.segments.last()
            && (terminal_path(&expr.path, &self.aliases)
                || self
                    .aliases
                    .get(&segment.ident.to_string())
                    .is_some_and(|name| terminal_api(name)))
        {
            self.add(segment.ident.span(), "terminal-reference", &segment.ident);
        }
        syn::visit::visit_expr_path(self, expr);
    }

    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if self.file == "firmware/probes/esp32s31-rx-chain/src/diagnostic.rs"
            && matches!(&*expr.func, syn::Expr::Path(path) if path.path.is_ident("record"))
            && let Some(syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(code),
                ..
            })) = expr.args.first()
        {
            self.add(code.span(), "retained-code", code);
        }
        syn::visit::visit_expr_call(self, expr);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        aliases(&item.tree, Vec::new(), &mut self.aliases);
    }

    fn visit_expr_method_call(&mut self, expr: &'ast syn::ExprMethodCall) {
        if self.file.starts_with("firmware/") && expr.method == "reset" {
            self.add(expr.method.span(), "reset-method", &expr.method);
        }
        if ["request_system_reset", "trigger_sys_ctrl_sys_sw_rst"]
            .contains(&expr.method.to_string().as_str())
        {
            self.add(expr.method.span(), "reset-trigger", &expr.method);
        }
        if self.file.starts_with("firmware/")
            && ["unwrap", "expect"].contains(&expr.method.to_string().as_str())
        {
            self.add(expr.method.span(), "panic-method", &expr.method);
        }
        syn::visit::visit_expr_method_call(self, expr);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Some(name) = mac.path.segments.last() {
            let text = name.ident.to_string();
            let text = self.aliases.get(&text).cloned().unwrap_or(text);
            if self.image_panic_scope() && panic_macro(&text) {
                self.add(name.ident.span(), "panic-macro", &name.ident);
            } else if ["retained_static", "__retained_static"].contains(&text.as_str()) {
                self.add(name.ident.span(), "retained-slot", &name.ident);
            } else if [
                "entry",
                "panic_handler",
                "phy_fault_reporter",
                "__exception_handler",
            ]
            .contains(&text.as_str())
            {
                self.add(name.ident.span(), "terminal-binding", &name.ident);
            }
        }
        self.macro_tokens(mac.tokens.clone());
    }
}

fn numeric_pattern(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Int(_),
            ..
        })
        | syn::Pat::Range(_) => true,
        syn::Pat::Tuple(tuple) => tuple.elems.iter().all(numeric_pattern),
        _ => false,
    }
}

fn pattern_name(pat: &syn::Pat) -> String {
    match pat {
        syn::Pat::Lit(syn::ExprLit {
            lit: syn::Lit::Int(value),
            ..
        }) => value.to_string(),
        syn::Pat::Tuple(tuple) => tuple
            .elems
            .iter()
            .map(pattern_name)
            .collect::<Vec<_>>()
            .join(","),
        syn::Pat::Range(_) => "range".into(),
        _ => unreachable!("only numeric patterns are recorded"),
    }
}

fn modules(
    items: &[syn::Item],
    module_dir: &Path,
    path_dir: &Path,
    scope: &[String],
    found: &mut Vec<(PathBuf, Option<PathBuf>, Vec<String>)>,
) {
    for item in items {
        if let syn::Item::Macro(item) = item
            && !test_only(&item.attrs)
            && item.mac.path.is_ident("include")
            && let Ok(path) = syn::parse2::<syn::LitStr>(item.mac.tokens.clone())
        {
            found.push((
                path_dir.join(path.value()),
                Some(module_dir.to_path_buf()),
                scope.to_vec(),
            ));
        }
        if let syn::Item::Mod(item) = item {
            if test_only(&item.attrs) {
                continue;
            }
            let mut scope = scope.to_vec();
            scope.push(item.ident.to_string());
            if let Some((_, items)) = &item.content {
                let directory = module_dir.join(item.ident.to_string());
                modules(items, &directory, &directory, &scope, found);
            } else {
                let explicit = item.attrs.iter().find_map(|attr| {
                    if let syn::Meta::NameValue(meta) = &attr.meta
                        && meta.path.is_ident("path")
                        && let syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(path),
                            ..
                        }) = &meta.value
                    {
                        Some(path_dir.join(path.value()))
                    } else {
                        None
                    }
                });
                let path = explicit.unwrap_or_else(|| {
                    let file = module_dir.join(format!("{}.rs", item.ident));
                    if file.exists() {
                        file
                    } else {
                        module_dir.join(item.ident.to_string()).join("mod.rs")
                    }
                });
                // Generated modules are outside this source inventory.
                if path.is_file() && !path.ends_with("pac/src/generated.rs") {
                    found.push((path, None, scope));
                }
            }
        }
    }
}

fn scan(root: &Path) -> ScannedSites {
    let root = root.canonicalize().expect("source root");
    let mut files = Vec::new();
    for directory in ["chips", "firmware", "portable", "protocols"] {
        sources(&root.join(directory), &mut files);
    }
    let packages: BTreeSet<_> = files
        .iter()
        .filter_map(|path| {
            path.ancestors()
                .skip(1)
                .find(|directory| directory.join("Cargo.toml").is_file())
                .map(Path::to_path_buf)
        })
        .collect();
    let mut pending = Vec::new();
    for package in packages {
        let manifest: toml::Table = toml::from_str(
            &std::fs::read_to_string(package.join("Cargo.toml")).expect("package manifest"),
        )
        .expect("manifest parses");
        for name in ["src/lib.rs", "src/main.rs"] {
            let file = package.join(name);
            if file.is_file() {
                let directory = file.parent().expect("crate root directory").to_path_buf();
                pending.push((file, Some(directory), vec!["crate".into()]));
            }
        }
        if let Some(path) = manifest
            .get("lib")
            .and_then(|lib| lib.get("path"))
            .and_then(toml::Value::as_str)
        {
            let file = package.join(path);
            let directory = file.parent().expect("crate root directory").to_path_buf();
            pending.push((file, Some(directory), vec!["crate".into()]));
        }
        if let Some(bins) = manifest.get("bin").and_then(toml::Value::as_array) {
            for bin in bins {
                if let Some(path) = bin.get("path").and_then(toml::Value::as_str) {
                    let file = package.join(path);
                    let directory = file.parent().expect("crate root directory").to_path_buf();
                    pending.push((file, Some(directory), vec!["crate".into()]));
                }
            }
        }
    }
    let mut sites = ScannedSites::new();
    let mut visited = BTreeSet::new();
    while let Some((path, included_module_dir, scope)) = pending.pop() {
        let path = path.canonicalize().expect("module path");
        if !visited.insert((path.clone(), scope.clone())) {
            continue;
        }
        let file = path
            .strip_prefix(&root)
            .expect("source below root")
            .to_string_lossy();
        let source = std::fs::read_to_string(&path).expect("read Rust source");
        let parsed = syn::parse_file(&source).unwrap_or_else(|error| panic!("{file}: {error}"));
        if test_only(&parsed.attrs) {
            continue;
        }
        let parent = path.parent().expect("source parent");
        let module_dir = if let Some(directory) = included_module_dir {
            directory
        } else if ["lib.rs", "main.rs", "mod.rs"]
            .iter()
            .any(|name| path.ends_with(name))
        {
            parent.to_path_buf()
        } else {
            parent.join(path.file_stem().expect("source stem"))
        };
        modules(&parsed.items, &module_dir, parent, &scope, &mut pending);
        let mut imports = Imports::default();
        imports.visit_file(&parsed);
        let mut scanner = Scanner {
            file: &file,
            source: &source,
            item: scope,
            ordinals: BTreeMap::new(),
            sites: ScannedSites::new(),
            decoder: false,
            aliases: imports.0,
        };
        scanner.visit_file(&parsed);
        sites.extend(scanner.sites);
    }
    sites
}

fn registry(text: &str) -> Result<Sites, String> {
    let document: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    if document.get("version").and_then(toml::Value::as_integer) != Some(1) {
        return Err("terminal registry version must be 1".into());
    }
    let entries = document
        .get("sites")
        .and_then(toml::Value::as_table)
        .ok_or("missing sites table")?;
    let mut sites = Sites::new();
    for (id, value) in entries {
        let entry = value.as_table().ok_or("site must be a table")?;
        let owner = entry
            .get("owner")
            .and_then(toml::Value::as_str)
            .ok_or("site needs an owner")?;
        if admitted_owner(id) != Some(owner) {
            return Err(format!("{id}: unadmitted reset owner {owner}"));
        }
        let classes = entry
            .get("classes")
            .and_then(toml::Value::as_array)
            .ok_or("site needs classes")?;
        if classes.is_empty()
            || classes
                .iter()
                .any(|class| !class.as_str().is_some_and(|class| CLASSES.contains(&class)))
        {
            return Err(format!("{id}: unknown or empty fault classes"));
        }
        if classes
            .iter()
            .any(|class| class.as_str() == Some("hardware-impossible"))
            && entry
                .get("proof")
                .and_then(toml::Value::as_str)
                .is_none_or(|proof| {
                    document
                        .get("proofs")
                        .and_then(|proofs| proofs.get(proof))
                        .and_then(toml::Value::as_str)
                        .is_none_or(|text| !text.starts_with("SOURCE:"))
                })
        {
            return Err(format!(
                "{id}: hardware impossibility needs a bounded SOURCE proof"
            ));
        }
        sites.insert(id.clone());
    }
    if let Some(nonterminal) = document.get("nonterminal").and_then(toml::Value::as_table) {
        for (id, reason) in nonterminal {
            if !id.rsplit_once(':').is_some_and(|(site, ordinal)| {
                site.ends_with(":reset-method:reset")
                    && ordinal.parse::<usize>().is_ok_and(|ordinal| ordinal > 0)
            }) || reason
                .as_str()
                .is_none_or(|reason| reason.trim().is_empty())
                || !sites.insert(id.clone())
            {
                return Err(format!("{id}: invalid nonterminal reset-method review"));
            }
        }
    }
    Ok(sites)
}

fn admitted_owner(id: &str) -> Option<&str> {
    let (file, _) = id.split_once(':')?;
    let (home, _) = file.split_once("/src/")?;
    if [
        "chips/esp32s31/hal",
        "chips/esp32s31/compose",
        "chips/espressif/staged-runtime",
        "chips/espressif/staged-bootstrap",
        "chips/espressif/executor",
    ]
    .contains(&home)
        || home.starts_with("firmware/")
    {
        Some(home)
    } else if home == "chips/espressif/ieee802154-engine"
        && REASON_ENUMS.iter().any(|&(path, _)| path == file)
    {
        Some("firmware/hil/esp32s31-phy")
    } else {
        None
    }
}

fn differences(actual: &ScannedSites, recorded: &Sites) -> Vec<String> {
    actual
        .iter()
        .filter(|(id, _)| !recorded.contains(*id))
        .map(|(id, location)| {
            let (file, _) = id.split_once(':').expect("site path and item");
            format!(
                "unregistered: {id} ({file}:{}:{})",
                location.line, location.column
            )
        })
        .chain(
            recorded
                .iter()
                .filter(|id| !actual.contains_key(*id))
                .map(|id| format!("stale: {id}")),
        )
        .collect()
}

#[test]
fn terminal_registry_matches_source() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let actual = scan(&root);
    let recorded =
        registry(include_str!("../terminal-sites.toml")).expect("valid terminal registry");
    assert_eq!(
        differences(&actual, &recorded),
        Vec::<String>::new(),
        "terminal-site registry drift"
    );
}

#[test]
fn missing_and_extra_sites_are_both_rejected() {
    let one = "firmware/fixture/src/main.rs:crate::one:panic-macro:panic:1";
    let two = "firmware/fixture/src/main.rs:crate::two:panic-macro:panic:1";
    let actual = ScannedSites::from([
        (
            one.into(),
            Location {
                line: 1,
                column: 12,
            },
        ),
        (
            two.into(),
            Location {
                line: 3,
                column: 12,
            },
        ),
    ]);
    assert_eq!(
        differences(&actual, &Sites::from([one.into()])),
        [format!(
            "unregistered: {two} (firmware/fixture/src/main.rs:3:12)"
        )]
    );
    assert_eq!(
        differences(
            &actual,
            &Sites::from([one.into(), two.into(), "gone".into()])
        ),
        ["stale: gone"]
    );
}

#[test]
fn scanner_finds_macros_callbacks_and_reasons_but_skips_tests_and_const_blocks() {
    let source = r#"
        // panic!("comment");
        const C: () = panic!("const");
        #[cfg(test)] mod tests { fn test() { panic!("test"); } }
        fn run() -> ! {
            const { panic!("compile time"); }
            let callback = runtime::reset_request::<Port, ()>;
            runtime::reset_recording::<Port>(|_| ());
            panic!("runtime");
        }
        macro_rules! entry { () => { #[panic_handler] fn panic() -> ! { runtime::reset_recording::<Port>(|_| ()); } }; }
    "#;
    let parsed = syn::parse_file(source).expect("fixture parses");
    let mut scanner = Scanner {
        file: "firmware/fixture/src/lib.rs",
        source,
        item: vec!["crate".into()],
        ordinals: BTreeMap::new(),
        sites: ScannedSites::new(),
        decoder: false,
        aliases: BTreeMap::new(),
    };
    scanner.visit_file(&parsed);
    assert_eq!(scanner.sites.len(), 6, "{:#?}", scanner.sites);
}

#[test]
fn metadata_requires_an_admitted_owner_and_fault_class() {
    for metadata in [
        "{ owner = 'protocols/wire', classes = ['rust-panic'] }",
        "{ owner = 'firmware/apps/boot-smoke', classes = [] }",
        "{ owner = 'firmware/apps/boot-smoke', classes = ['impossible-because-named-fault'] }",
        "{ owner = 'firmware/apps/boot-smoke', classes = ['hardware-impossible'] }",
        "{ owner = 'firmware/apps/radio-root', classes = ['rust-panic'] }",
    ] {
        assert!(registry(&format!("version = 1\n[sites]\n'firmware/apps/boot-smoke/src/main.rs:crate::run:panic-macro:panic:1' = {metadata}\n")).is_err());
    }
    assert!(registry("version = 1\n[sites]\n'firmware/apps/boot-smoke/src/main.rs:crate::run:panic-macro:panic:1' = { owner = 'firmware/apps/boot-smoke', classes = ['rust-panic'] }\n").is_ok());
}

#[test]
fn source_addition_deletion_and_file_removal_require_registry_changes() {
    let root = tempfile::tempdir().expect("source fixture");
    let package = root.path().join("firmware/apps/fixture");
    std::fs::create_dir_all(package.join("src")).expect("fixture source directory");
    std::fs::write(package.join("Cargo.toml"), "[package]\nname = 'fixture'\n")
        .expect("fixture manifest");
    std::fs::write(
        package.join("src/main.rs"),
        "include!(\"entry.rs\");\n#[cfg(test)] mod tests;\n",
    )
    .expect("root");
    std::fs::write(
        package.join("src/tests.rs"),
        "fn excluded() { panic!(\"test\"); }",
    )
    .expect("test module");
    let source = package.join("src/entry.rs");
    std::fs::write(
        &source,
        "fn end() -> ! { runtime::reset_recording::<Port>(|_| ()) }\n",
    )
    .expect("terminal source");
    let original_scan = scan(root.path());
    let original: Sites = original_scan.keys().cloned().collect();
    assert_eq!(
        original.len(),
        2,
        "literal include is scanned, test module is excluded"
    );
    assert!(differences(&original_scan, &original).is_empty());
    std::fs::write(
        &source,
        "\n    fn end() -> ! { runtime::reset_recording::<Port>(|_| ()) }\n",
    )
    .expect("shift line and column above the sites");
    let shifted = scan(root.path());
    assert!(differences(&shifted, &original).is_empty());
    assert_ne!(
        shifted, original_scan,
        "navigation coordinates stay current"
    );
    std::fs::write(&source, "fn end() -> ! { runtime::reset_recording::<Port>(|_| ()) }\nfn extra() { runtime::reset_recording::<Port>(|_| ()); }\n").expect("new site");
    let added = differences(&scan(root.path()), &original);
    assert_eq!(added.len(), 1);
    assert!(added[0].starts_with("unregistered:"));
    std::fs::write(&source, "fn end() -> ! { loop {} }\n").expect("remove call");
    let removed = differences(&scan(root.path()), &original);
    assert_eq!(removed.len(), 1);
    assert!(removed[0].starts_with("stale:"));
    std::fs::write(package.join("src/main.rs"), "fn main() {}\n").expect("remove inclusion");
    std::fs::remove_file(source).expect("remove terminal source");
    assert_eq!(differences(&scan(root.path()), &original).len(), 2);
}

#[test]
fn aliases_macro_expansions_and_new_retained_variants_are_inventoried() {
    let source = r#"
        use runtime::fatal::{reset_recording as reboot, reset as restart};
        use core::panic as fail;
        use oer_espressif_staged_runtime::{self as rt};
        use oer_espressif_staged_runtime as platform;
        fn run() { reboot::<Port>(|_| ()); restart::<Port>(); fail!("image"); rt::reset::<Port>(); platform::reset::<Port>(); }
        macro_rules! emitted { () => { runtime::fatal::reset::<Port>(); rt::reset::<Port>(); } }
        enum FaultReason { One = 1, Two = 2 }
    "#;
    let parsed = syn::parse_file(source).expect("fixture parses");
    let mut scanner = Scanner {
        file: "chips/espressif/staged-runtime/src/panic/fault.rs",
        source,
        item: vec!["crate".into()],
        ordinals: BTreeMap::new(),
        sites: ScannedSites::new(),
        decoder: false,
        aliases: BTreeMap::new(),
    };
    scanner.visit_file(&parsed);
    for name in [
        "terminal-reference:reboot",
        "terminal-reference:restart",
        "panic-macro:fail",
        "terminal-reference:reset",
        "retained-reason:FaultReason::One",
        "retained-reason:FaultReason::Two",
    ] {
        assert!(
            scanner
                .sites
                .keys()
                .any(|site| site.contains(&format!(":{name}:"))),
            "missing {name}"
        );
    }
    assert_eq!(scanner.sites.len(), 9);
}

#[test]
fn nonterminal_reviews_cannot_hide_panic_sites_or_survive_removed_calls() {
    let id = "firmware/apps/boot-smoke/src/main.rs:crate::run:reset-method:reset:1";
    let recorded = registry(&format!(
        "version = 1\n[sites]\n[nonterminal]\n'{id}' = 'parser clears its frame and returns'\n"
    ))
    .expect("reviewed method");
    assert_eq!(
        differences(&ScannedSites::new(), &recorded),
        [format!("stale: {id}")]
    );
    assert!(registry("version = 1\n[sites]\n[nonterminal]\n'firmware/apps/boot-smoke/src/main.rs:crate::run:panic-macro:panic:1' = 'hide panic'\n").is_err());
}

#[test]
fn duplicate_registry_keys_and_overlapping_nonterminal_reviews_are_rejected() {
    let id = "firmware/apps/boot-smoke/src/main.rs:crate::run:reset-method:reset:1";
    let entry = format!(
        "'{id}' = {{ owner = 'firmware/apps/boot-smoke', classes = ['requested-reset'] }}\n"
    );
    assert!(registry(&format!("version = 1\n[sites]\n{entry}{entry}")).is_err());
    assert!(
        registry(&format!(
            "version = 1\n[sites]\n{entry}[nonterminal]\n'{id}' = 'reviewed reset'\n"
        ))
        .is_err()
    );
}

#[test]
fn enclosing_items_distinguish_methods_and_number_only_identical_sites() {
    let source = r#"
        mod nested {
            impl<T> Device<T> {
                fn run() {
                    panic!("first");
                    runtime::reset_recording::<Port>(|_| ());
                    panic!("second");
                    fn inner() { panic!("nested function"); }
                }
            }
            impl Service for Device<u8> {
                fn run() { panic!("trait implementation"); }
            }
            impl Device<u16> {
                fn run() { panic!("different type"); }
            }
            macro_rules! emitted { () => { panic!("template"); } }
        }
    "#;
    let mut scanner = Scanner {
        file: "firmware/fixture/src/lib.rs",
        source,
        item: vec!["crate".into()],
        ordinals: BTreeMap::new(),
        sites: ScannedSites::new(),
        decoder: false,
        aliases: BTreeMap::new(),
    };
    scanner.visit_file(&syn::parse_file(source).expect("fixture parses"));
    let names: Sites = scanner.sites.into_keys().collect();
    let file = "firmware/fixture/src/lib.rs";
    assert_eq!(
        names,
        Sites::from([
            format!("{file}:crate::nested::impl Device < T >::run:panic-macro:panic:1"),
            format!("{file}:crate::nested::impl Device < T >::run:panic-macro:panic:2"),
            format!(
                "{file}:crate::nested::impl Device < T >::run:terminal-reference:reset_recording:1"
            ),
            format!("{file}:crate::nested::impl Device < T >::run::inner:panic-macro:panic:1"),
            format!(
                "{file}:crate::nested::impl Service for Device < u8 >::run:panic-macro:panic:1"
            ),
            format!("{file}:crate::nested::impl Device < u16 >::run:panic-macro:panic:1"),
            format!("{file}:crate::nested::emitted:panic-macro:panic:1"),
        ])
    );
}

#[test]
fn module_paths_follow_inline_modules_path_attributes_and_includes() {
    let root = tempfile::tempdir().expect("source fixture");
    let package = root.path().join("firmware/apps/fixture");
    let src = package.join("src");
    std::fs::create_dir_all(src.join("outer")).expect("module directory");
    std::fs::write(package.join("Cargo.toml"), "[package]\nname = 'fixture'\n")
        .expect("fixture manifest");
    std::fs::write(
        src.join("main.rs"),
        "mod outer; mod inline { fn run() { panic!(); } }",
    )
    .expect("crate root");
    std::fs::write(
        src.join("outer.rs"),
        "#[path = \"different.rs\"] mod named;",
    )
    .expect("out-of-line module");
    std::fs::write(src.join("different.rs"), "include!(\"included.rs\");")
        .expect("explicit module path");
    std::fs::write(src.join("included.rs"), "fn run() { panic!(); }").expect("included source");
    let actual = scan(root.path());
    assert_eq!(
        actual.into_keys().collect::<Sites>(),
        Sites::from([
            "firmware/apps/fixture/src/main.rs:crate::inline::run:panic-macro:panic:1".into(),
            "firmware/apps/fixture/src/included.rs:crate::outer::named::run:panic-macro:panic:1"
                .into(),
        ])
    );
}

#[test]
fn common_retained_classes_match_the_registry_vocabulary() {
    let source = syn::parse_file(include_str!(
        "../../../../portable/diagnostics/src/fault.rs"
    ))
    .expect("common fault codec parses");
    let classes = source
        .items
        .into_iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Class" => Some(item),
            _ => None,
        })
        .expect("the common codec declares its classes");
    let actual: BTreeSet<_> = classes
        .variants
        .into_iter()
        .map(|variant| {
            let mut spelling = String::new();
            for (index, ch) in variant.ident.to_string().chars().enumerate() {
                if index != 0 && ch.is_ascii_uppercase() {
                    spelling.push('-');
                }
                spelling.push(ch.to_ascii_lowercase());
            }
            spelling
        })
        .collect();
    assert_eq!(
        actual,
        CLASSES
            .iter()
            .map(|class| (*class).to_owned())
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn free_function_codecs_keep_their_numeric_reason_inventory() {
    let file = "chips/esp32s31/hal/src/domain/transaction/record.rs";
    let source = "fn report_fault() { match 2 { 1 => (), 2 => (), _ => () } }";
    let mut scanner = Scanner {
        file,
        source,
        item: vec!["crate".into()],
        ordinals: BTreeMap::new(),
        sites: ScannedSites::new(),
        decoder: false,
        aliases: BTreeMap::new(),
    };
    scanner.visit_file(&syn::parse_file(source).expect("free codec parses"));
    assert_eq!(
        scanner.sites.into_keys().collect::<Sites>(),
        Sites::from([
            format!("{file}:crate::report_fault:retained-code:1:1"),
            format!("{file}:crate::report_fault:retained-code:2:1"),
        ])
    );
}

#[test]
fn common_exception_binding_and_boot_callbacks_are_inventoried() {
    let source = r#"
        use platform::fatal::boot_failure as end_boot;
        fn check() { let callback = end_boot::<Port>; consume(callback); }
        macro_rules! binding { () => {
            __exception_handler!(Port, supplemental);
            trap::handle::<Port>(frame, supplemental)
        } }
    "#;
    let mut scanner = Scanner {
        file: "firmware/fixture/src/lib.rs",
        source,
        item: vec!["crate".into()],
        ordinals: BTreeMap::new(),
        sites: ScannedSites::new(),
        decoder: false,
        aliases: BTreeMap::new(),
    };
    scanner.visit_file(&syn::parse_file(source).expect("fixture parses"));
    let keys: Sites = scanner.sites.into_keys().collect();
    assert!(
        keys.iter()
            .any(|key| key.contains("terminal-reference:end_boot"))
    );
    assert!(
        keys.iter()
            .any(|key| key.contains("terminal-binding:__exception_handler"))
    );
    assert!(
        keys.iter()
            .any(|key| key.contains("terminal-reference:handle"))
    );
}
