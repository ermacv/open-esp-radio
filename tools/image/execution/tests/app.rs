//! Cross-check the candidate model against independent current sources. No
//! firmware is built and no MMIO owner is constructed by these host tests.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use oer_chips_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu1Irq, FromCpu2Irq, FromCpu3Irq},
    system_timer::{SystimerAlarm0, SystimerAlarm1, SystimerAlarm2},
    timg0_alarm::Timg0AlarmInterrupt,
    timg1_alarm::{Timg1AlarmInterrupt, Timg1Timer1Interrupt},
    usb_serial_jtag::ConsoleInterrupt,
};
use oer_chips_esp32s31_radio_ieee80211_backend::irq::WifiIrqPart;
use oer_chips_family_facts::{Facts, generate};
use oer_portable_mmio::InterruptSources;
use oer_tools_image_execution::{
    Class, Configuration, ContextKind, Hardware, Model, NumberKind, Raiser, Resolved,
};
use quote::ToTokens;
use syn::{Token, parse::Parse, punctuated::Punctuated};

const APP: &str = "chips/esp32s31/app";
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}
fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap()
}
fn model() -> Model {
    Model::parse(&read(&format!("{APP}/execution.toml"))).unwrap()
}
fn inventory() -> toml::Table {
    toml::from_str(&read(&format!("{APP}/execution-inventory.toml"))).unwrap()
}
fn text<'a>(row: &'a toml::Value, key: &str) -> &'a str {
    row[key].as_str().unwrap()
}
fn rows<'a>(inventory: &'a toml::Table, table: &str) -> &'a [toml::Value] {
    inventory[table].as_array().unwrap()
}
fn names<'a>(values: impl Iterator<Item = &'a str>) -> BTreeSet<&'a str> {
    values.collect()
}
fn tokens(value: &impl ToTokens) -> String {
    value.to_token_stream().to_string()
}

fn part_sources() -> [(&'static str, &'static [u16]); 11] {
    [
        ("wake", <FromCpu0Irq as InterruptSources>::SOURCES),
        ("alarm", <SystimerAlarm1 as InterruptSources>::SOURCES),
        ("radio_alarm", <SystimerAlarm0 as InterruptSources>::SOURCES),
        ("phy", <Timg0AlarmInterrupt as InterruptSources>::SOURCES),
        ("radio", <FromCpu2Irq as InterruptSources>::SOURCES),
        // The real aggregate's impl<A> InterruptSources is independent of
        // its doorbell acknowledger. The target-only alias chain is pinned
        // by a separate syntax check below; handler typing stays in Rust.
        ("wifi", <WifiIrqPart<()> as InterruptSources>::SOURCES),
        (
            "console_usb",
            <ConsoleInterrupt as InterruptSources>::SOURCES,
        ),
        (
            "sampler",
            <Timg1AlarmInterrupt as InterruptSources>::SOURCES,
        ),
        (
            "sampler1",
            <Timg1Timer1Interrupt as InterruptSources>::SOURCES,
        ),
        ("hart1_wake", <FromCpu1Irq as InterruptSources>::SOURCES),
        ("hart1_alarm", <SystimerAlarm2 as InterruptSources>::SOURCES),
    ]
}
fn facts() -> Facts {
    generate::tracked("esp32s31").unwrap().0
}
fn hardware(facts: &Facts) -> Hardware<'_> {
    let chip = oer_tools_registers_model::Chip::load(&root(), "esp32s31").unwrap();
    Hardware {
        chip: &facts.id,
        interrupts: facts.interrupts.as_ref().unwrap(),
        sources: chip
            .model
            .interrupts()
            .iter()
            .map(|source| (source.name.clone(), source.source))
            .collect(),
        software: [
            FromCpu0Irq::SOURCES[0],
            FromCpu1Irq::SOURCES[0],
            FromCpu2Irq::SOURCES[0],
            FromCpu3Irq::SOURCES[0],
        ]
        .into_iter()
        .enumerate()
        .map(|(n, source)| (n as u8, source))
        .collect(),
    }
}

// This evaluates the Rust cfg representation independently of the model's
// all/any/none feature predicate. No arbitrary cfg expressions are guessed.
fn cfg_matches(cfg: &str, config: &Configuration) -> bool {
    fn eval(meta: &syn::Meta, config: &Configuration) -> bool {
        match meta {
            syn::Meta::NameValue(value) if value.path.is_ident("feature") => {
                let syn::Expr::Lit(lit) = &value.value else {
                    panic!("feature literal")
                };
                let syn::Lit::Str(name) = &lit.lit else {
                    panic!("feature string")
                };
                config.features.contains(&name.value())
            }
            syn::Meta::List(list) => {
                let args = list
                    .parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated)
                    .unwrap();
                if list.path.is_ident("all") {
                    args.iter().all(|arg| eval(arg, config))
                } else if list.path.is_ident("any") {
                    args.iter().any(|arg| eval(arg, config))
                } else if list.path.is_ident("not") {
                    assert_eq!(args.len(), 1);
                    !eval(args.first().unwrap(), config)
                } else {
                    panic!("unsupported cfg: {}", tokens(meta))
                }
            }
            _ => panic!("unsupported cfg: {}", tokens(meta)),
        }
    }
    cfg.is_empty() || eval(&syn::parse_str(cfg).unwrap(), config)
}
fn inventory_names<'a>(
    inventory: &'a toml::Table,
    table: &str,
    field: &str,
    config: &Configuration,
) -> BTreeSet<&'a str> {
    names(
        rows(inventory, table)
            .iter()
            .filter(|row| row.get("scope").and_then(toml::Value::as_str) != Some("probes only"))
            .filter(|row| {
                cfg_matches(
                    row.get("cfg").and_then(toml::Value::as_str).unwrap_or(""),
                    config,
                )
            })
            .map(|row| text(row, field)),
    )
}

struct TableEntry {
    name: String,
    token: String,
    cfg: Vec<syn::Meta>,
    part: syn::Type,
    handler: syn::Path,
    class: syn::Expr,
    layout: String,
}
struct Table(Vec<TableEntry>);
impl Parse for Table {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        input.parse::<Token![@]>()?;
        let contexts: syn::Ident = input.parse()?;
        assert_eq!(contexts, "contexts");
        let mut entries = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(syn::Attribute::parse_outer)?;
            let cfg = attrs
                .iter()
                .filter(|a| a.path().is_ident("cfg"))
                .map(|a| a.parse_args::<syn::Meta>())
                .collect::<syn::Result<Vec<_>>>()?;
            let name: syn::Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let token: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let part = input.parse()?;
            input.parse::<Token![=>]>()?;
            let handler = input.parse()?;
            input.parse::<Token![,]>()?;
            input.parse::<Token![@]>()?;
            let layout: syn::Ident = input.parse()?;
            assert_eq!(layout, "layout");
            let content;
            syn::parenthesized!(content in input);
            let layout: syn::Ident = content.parse()?;
            assert!(content.is_empty());
            input.parse::<Token![,]>()?;
            let class = input.parse()?;
            input.parse::<Token![;]>()?;
            entries.push(TableEntry {
                name: name.to_string(),
                token: token.to_string(),
                cfg,
                part,
                handler,
                class,
                layout: layout.to_string(),
            });
        }
        Ok(Self(entries))
    }
}
fn table() -> Table {
    let file = syn::parse_file(&read(&format!("{APP}/src/interrupt_table.in"))).unwrap();
    let syn::Item::Macro(item) = file.items.first().unwrap() else {
        panic!("table macro")
    };
    syn::parse2(item.mac.tokens.clone()).unwrap()
}
fn class(expr: &syn::Expr) -> (Class, String) {
    match expr {
        syn::Expr::Path(path) if path.path.is_ident("Hardware") => (Class::Hardware, String::new()),
        syn::Expr::Path(path) if path.path.is_ident("Executor") => (Class::Executor, String::new()),
        syn::Expr::Call(call) => {
            assert_eq!(tokens(&call.func), "Exempt");
            let syn::Expr::Path(kind) = call.args.first().unwrap() else {
                panic!("exemption kind")
            };
            let kind = if kind.path.is_ident("ExecutorLevel") {
                Class::ExemptExecutorLevel
            } else {
                assert!(kind.path.is_ident("WakerCall"));
                Class::ExemptWakerCall
            };
            let syn::Expr::Lit(reason) = call.args.last().unwrap() else {
                panic!("reason")
            };
            let syn::Lit::Str(reason) = &reason.lit else {
                panic!("reason string")
            };
            (kind, reason.value())
        }
        _ => panic!("unknown class"),
    }
}

// Interpret only the executor aliases currently used by app signal statics.
// Unknown alias shapes fail instead of being assigned guessed scheduling data.
fn type_args(ty: &syn::Type) -> (&syn::Ident, Vec<&syn::Type>) {
    let syn::Type::Path(path) = ty else {
        panic!("type path")
    };
    let last = path.path.segments.last().unwrap();
    let args = match &last.arguments {
        syn::PathArguments::AngleBracketed(args) => args
            .args
            .iter()
            .map(|arg| {
                let syn::GenericArgument::Type(ty) = arg else {
                    panic!("type argument")
                };
                ty
            })
            .collect(),
        syn::PathArguments::None => Vec::new(),
        _ => panic!("generic type arguments"),
    };
    (&last.ident, args)
}
fn alias_identity(name: &str) -> (u8, ContextKind) {
    let file = match name {
        "DriverExecutor" => "chips/esp32s31/radio/ieee80211/station/src/chip.rs".to_owned(),
        "AppExecutor" | "Hart1Executor" => format!("{APP}/src/hart1.rs"),
        "RadioExecutor" => format!("{APP}/src/wifi.rs"),
        _ => panic!("unknown executor alias {name}"),
    };
    let parsed = syn::parse_file(&read(&file)).unwrap();
    let alias = parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Type(alias) if alias.ident == name => Some(alias),
            _ => None,
        })
        .unwrap();
    let syn::Type::Path(path) = &*alias.ty else {
        panic!("executor alias path")
    };
    let segment = path.path.segments.last().unwrap();
    let kind = if segment.ident == "Executor" {
        ContextKind::Thread
    } else {
        assert_eq!(segment.ident, "InterruptExecutor");
        ContextKind::InterruptExecutor
    };
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        panic!("executor arguments")
    };
    let syn::GenericArgument::Const(syn::Expr::Lit(number)) = args.args.iter().nth(1).unwrap()
    else {
        panic!("SWI const")
    };
    let syn::Lit::Int(number) = &number.lit else {
        panic!("SWI integer")
    };
    (number.base10_parse().unwrap(), kind)
}
fn executor_name(ty: &syn::Type, model: &Resolved) -> String {
    let (alias, args) = type_args(ty);
    assert!(args.is_empty());
    let (swi, kind) = alias_identity(&alias.to_string());
    model
        .executors
        .iter()
        .find(|executor| {
            executor.swi == swi
                && model
                    .contexts
                    .iter()
                    .any(|context| context.name == executor.context && context.kind == kind)
        })
        .unwrap()
        .name
        .clone()
}
fn entry_name(ty: &syn::Type, table: &Table) -> String {
    let (token, args) = type_args(ty);
    assert!(args.is_empty());
    table
        .0
        .iter()
        .find(|entry| *token == entry.token)
        .unwrap()
        .name
        .clone()
}
fn raisers(ty: &syn::Type, model: &Resolved, table: &Table) -> Vec<Raiser> {
    if let syn::Type::Tuple(tuple) = ty {
        return tuple
            .elems
            .iter()
            .flat_map(|ty| raisers(ty, model, table))
            .collect();
    }
    let (kind, args) = type_args(ty);
    let kind = kind.to_string();
    let from = match kind.as_str() {
        "FromHandler" | "FromExecutorLevel" => {
            assert_eq!(args.len(), 1);
            let entry = entry_name(args[0], table);
            if kind == "FromHandler" {
                Raiser::Handler { entry }
            } else {
                Raiser::ExecutorLevel { entry }
            }
        }
        "FromTask" | "FromOwnTask" | "FromPeerHart" => {
            assert_eq!(args.len(), 2);
            let executor = executor_name(args[0], model);
            let entry = entry_name(args[1], table);
            assert_eq!(
                model
                    .executors
                    .iter()
                    .find(|x| x.name == executor)
                    .unwrap()
                    .entry,
                entry
            );
            match kind.as_str() {
                "FromTask" => Raiser::Task { executor },
                "FromOwnTask" => Raiser::OwnTask { executor },
                _ => Raiser::PeerHart { executor },
            }
        }
        _ => panic!("unknown raiser {kind}"),
    };
    vec![from]
}
fn assert_signal_bindings(model: &Resolved, inventory: &toml::Table, table: &Table) {
    for signal in &model.signals {
        // These two generic factories have concrete composition bindings
        // below; the other seven signals are concrete placed statics.
        if matches!(signal.name.as_str(), "pilot reply" | "console") {
            continue;
        }
        let row = rows(inventory, "signal")
            .iter()
            .find(|row| text(row, "name") == signal.name)
            .unwrap();
        let source = read(text(row, "site"));
        let line = source
            .lines()
            .find(|line| line.contains(text(row, "anchor")))
            .unwrap();
        let declaration = line[line.find("static ").unwrap()..]
            .split_once(", in ")
            .unwrap()
            .0;
        let static_ = syn::parse_str::<syn::ItemStatic>(&format!("{declaration};")).unwrap();
        let (_, args) = type_args(&static_.ty);
        assert_eq!(args.len(), 1);
        assert_eq!(executor_name(args[0], model), signal.target);
        let syn::Expr::Call(call) = &*static_.expr else {
            panic!("signal constructor")
        };
        let syn::Expr::Path(path) = &*call.func else {
            panic!("signal constructor path")
        };
        let new = path.path.segments.last().unwrap();
        assert_eq!(new.ident, "new");
        let syn::PathArguments::AngleBracketed(args) = &new.arguments else {
            panic!("signal constructor arguments")
        };
        let args: Vec<_> = args
            .args
            .iter()
            .map(|arg| {
                let syn::GenericArgument::Type(ty) = arg else {
                    panic!("signal constructor type")
                };
                ty
            })
            .collect();
        assert_eq!(args.len(), 2);
        assert_eq!(raisers(args[0], model, table), signal.raisers);
        assert_eq!(
            entry_name(args[1], table),
            model
                .executors
                .iter()
                .find(|x| x.name == signal.target)
                .unwrap()
                .entry
        );
    }
    if let Some(pilot) = model
        .signals
        .iter()
        .find(|signal| signal.name == "pilot reply")
    {
        assert_eq!(pilot.target, "radio");
        assert_eq!(
            pilot.raisers,
            [Raiser::ExecutorLevel {
                entry: "phy".into()
            }]
        );
        assert!(read(&format!("{APP}/src/wifi.rs")).contains(
            "static PILOT: pilot::Storage<PhyToken> = pilot::Storage::new::<RadioToken>()"
        ));
        assert!(
            read(&format!("{APP}/src/pilot.rs"))
                .contains("reply: IrqSignal::new::<FromExecutorLevel<I>, R>()")
        );
    }
    if let Some(console) = model.signals.iter().find(|signal| signal.name == "console") {
        assert_eq!(console.target, "application");
        assert_eq!(
            console.raisers,
            [Raiser::Handler {
                entry: "console_usb".into()
            }]
        );
        assert!(
            read(&format!("{APP}/src/console.rs"))
                .contains("static BINDING: Binding = Binding::new::<ConsoleToken, WakeToken>()")
        );
    }
}

fn assert_inventory(model: &Resolved, inventory: &toml::Table) {
    let config = &model.configuration;
    assert_eq!(
        names(model.entries.iter().map(|r| r.declaration.name.as_str())),
        inventory_names(inventory, "interrupt", "name", config)
    );
    assert_eq!(
        names(model.executors.iter().map(|r| r.name.as_str())),
        inventory_names(inventory, "executor", "name", config)
    );
    assert_eq!(
        names(model.task_sets.iter().map(|r| r.name.as_str())),
        inventory_names(inventory, "task-set", "name", config)
    );
    assert_eq!(
        names(model.task_roots.iter().map(|r| r.name.as_str())),
        inventory_names(inventory, "task-root", "name", config)
    );
    assert_eq!(
        names(model.signals.iter().map(|r| r.name.as_str())),
        inventory_names(inventory, "signal", "name", config)
    );
    assert_eq!(
        names(model.peer_notifies.iter().map(|r| r.name.as_str())),
        inventory_names(inventory, "peer-notify", "name", config)
    );
    assert_eq!(
        names(model.timers.iter().map(|r| r.inventory.as_str())),
        inventory_names(inventory, "timer", "name", config)
    );
    assert_eq!(
        names(model.hart_start.iter().map(|r| r.step.as_str())),
        inventory_names(inventory, "hart-start", "step", config)
    );
    assert_eq!(
        names(model.waits.iter().map(|r| r.id.as_str())),
        inventory_names(inventory, "wait", "id", config)
    );
    for executor in &model.executors {
        let row = rows(inventory, "executor")
            .iter()
            .find(|r| text(r, "name") == executor.name)
            .unwrap();
        assert_eq!(executor.entry, text(row, "entry"));
        assert_eq!(executor.swi, text(row, "swi").parse::<u8>().unwrap());
    }
    for set in &model.task_sets {
        let row = rows(inventory, "task-set")
            .iter()
            .find(|r| text(r, "name") == set.name)
            .unwrap();
        let owner = model
            .executors
            .iter()
            .find(|e| e.name == text(row, "executor"))
            .unwrap();
        assert_eq!(set.context, owner.context);
        assert_eq!(set.observer, text(row, "observer").parse::<u8>().unwrap());
    }
    for peer in &model.peer_notifies {
        let row = rows(inventory, "peer-notify")
            .iter()
            .find(|r| text(r, "name") == peer.name)
            .unwrap();
        assert_eq!(peer.signal, text(row, "signal"));
    }
    for wait in &model.waits {
        let row = rows(inventory, "wait")
            .iter()
            .find(|r| text(r, "id") == wait.id)
            .unwrap();
        assert_eq!(wait.service, text(row, "condition"));
        assert_eq!(wait.held, text(row, "held"));
        assert_eq!(wait.bound.detail, text(row, "bound"));
    }
    for use_ in &model.number_uses {
        let row = rows(inventory, "number-site")
            .iter()
            .find(|r| text(r, "id") == use_.id)
            .unwrap();
        let expected_kind = match use_.kind {
            NumberKind::Swi => "swi",
            NumberKind::Hart => "hart",
            NumberKind::Observer => "observer",
        };
        assert_eq!(expected_kind, text(row, "kind"));
        let source = read(text(row, "site"));
        let line = source
            .lines()
            .find(|l| l.contains(text(row, "anchor")))
            .unwrap();
        let tail = match use_.kind {
            NumberKind::Swi => line.split_once("Esp32s31, ").unwrap().1,
            NumberKind::Observer => line.split_once("executor_observer::<").unwrap().1,
            NumberKind::Hart => line
                .split_once(".run::<")
                .unwrap()
                .1
                .split(',')
                .nth(4)
                .unwrap()
                .trim_start(),
        };
        let actual = tail
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u8>()
            .unwrap();
        assert_eq!(model.number_value(use_).unwrap(), actual, "{}", use_.id);
    }
}

#[test]
fn every_feature_configuration_matches_inventory_layout_rust_table_and_typed_sources() {
    let model = model();
    let inventory = inventory();
    let facts = facts();
    let hardware = hardware(&facts);
    let table = table();
    let parts: BTreeMap<_, _> = part_sources().into_iter().collect();
    assert_eq!(
        model.features,
        names(
            toml::from_str::<toml::Table>(&read(&format!("{APP}/Cargo.toml"))).unwrap()["features"]
                .as_table()
                .unwrap()
                .keys()
                .map(String::as_str)
        )
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(model.configurations().unwrap().len(), 32);
    assert_eq!(
        parts.keys().copied().collect::<BTreeSet<_>>(),
        table.0.iter().map(|e| e.name.as_str()).collect()
    );
    let layout = facts.interrupt_layout.as_ref().unwrap();
    let mut seen_numbers = BTreeSet::new();
    for config in model.configurations().unwrap() {
        assert_eq!(
            config.features.contains("observe"),
            config.instrumentation.contains("trace")
        );
        let resolved = model.resolve(&config, &hardware).unwrap();
        assert_inventory(&resolved, &inventory);
        assert_signal_bindings(&resolved, &inventory, &table);
        let selected: Vec<_> = table
            .0
            .iter()
            .filter(|e| e.cfg.iter().all(|c| cfg_matches(&tokens(c), &config)))
            .collect();
        assert_eq!(resolved.entries.len(), selected.len());
        for (entry, source) in resolved.entries.iter().zip(selected) {
            assert_eq!(entry.declaration.name, source.name);
            assert_eq!(source.layout, source.name);
            assert_eq!(
                tokens(&syn::parse_str::<syn::Type>(&entry.declaration.part).unwrap()),
                tokens(&source.part)
            );
            assert_eq!(
                tokens(&syn::parse_str::<syn::Path>(&entry.declaration.handler).unwrap()),
                tokens(&source.handler)
            );
            assert_eq!(
                (entry.declaration.class, entry.declaration.reason.clone()),
                class(&source.class)
            );
            assert_eq!(entry.source_ids, parts[entry.declaration.name.as_str()]);
            let location = layout
                .entries
                .iter()
                .find(|e| e.name == entry.declaration.name)
                .unwrap();
            assert_eq!(entry.hart, location.hart);
            assert_eq!(entry.declaration.line, location.line);
            assert_eq!(entry.level, layout.contexts[&location.context]);
        }
        assert_eq!(
            resolved.harts,
            if config.features.contains("group") {
                BTreeSet::from([0, 1])
            } else {
                BTreeSet::from([0])
            }
        );
        seen_numbers.extend(resolved.number_uses.iter().map(|r| r.id.clone()));
        // Optional instrumentation never changes placement or record selection.
        let mut traced = config.clone();
        traced.instrumentation.insert("trace".into());
        let mut traced_resolved = model.resolve(&traced, &hardware).unwrap();
        traced_resolved.configuration = config.clone();
        assert_eq!(traced_resolved, resolved);
        // Runtime choices remain after feature resolution; app timer 1 has
        // exactly one installation path in either valid radio state.
        let states = if config.features.contains("wifi") {
            vec![
                BTreeSet::new(),
                BTreeSet::from(["radio-started".into()]),
                BTreeSet::from(["radio-started".into(), "network-stopping".into()]),
            ]
        } else {
            vec![BTreeSet::new()]
        };
        for active in states {
            resolved.validate_runtime(&active).unwrap();
            let app_timers: Vec<_> = resolved
                .timers
                .iter()
                .filter(|t| t.entry == "alarm" && t.active_when.matches(&active))
                .collect();
            assert_eq!(app_timers.len(), 1);
            let risks = resolved.wait_risks(&active).unwrap();
            assert!(risks.iter().any(|w| w.id == "application-send"));
            assert!(risks.iter().any(|w| w.id == "application-receive"));
            assert!(!risks.iter().any(|w| w.id == "network-stop"));
            assert_eq!(
                risks.iter().any(|w| w.id == "console-read"),
                config.features.contains("console")
            );
        }
        let serialized = toml::to_string(&resolved).unwrap();
        assert!(
            !serialized
                .lines()
                .any(|l| l.starts_with("when =") || l.ends_with(".when]"))
        );
    }
    assert_eq!(
        seen_numbers,
        rows(&inventory, "number-site")
            .iter()
            .map(|r| text(r, "id").to_owned())
            .collect()
    );
}

#[test]
fn wifi_host_part_follows_the_target_only_alias_chain() {
    let app = syn::parse_file(&read(&format!("{APP}/src/wifi.rs"))).unwrap();
    let alias = app
        .items
        .iter()
        .find_map(|i| match i {
            syn::Item::Type(t) if t.ident == "WifiPart" => Some(&t.ty),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        tokens(alias),
        tokens(&syn::parse_str::<syn::Type>("<wifi::Interrupts as InterruptTypes>::Part").unwrap())
    );
    let source = read("chips/esp32s31/radio/ieee80211/station/src/chip.rs");
    let chip = syn::parse_file(&source).unwrap();
    let implementation = chip
        .items
        .iter()
        .find_map(|i| match i {
            syn::Item::Impl(i)
                if i.trait_
                    .as_ref()
                    .is_some_and(|(_, p, _)| p.is_ident("InterruptTypes")) =>
            {
                Some(i)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(tokens(&implementation.self_ty), "Interrupts");
    let part = implementation
        .items
        .iter()
        .find_map(|i| match i {
            syn::ImplItem::Type(t) if t.ident == "Part" => Some(&t.ty),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        tokens(part),
        tokens(&syn::parse_str::<syn::Type>("WifiIrqPart<Lower>").unwrap())
    );
    let imports = source
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    assert!(imports.contains("useoer_chips_esp32s31_radio_ieee80211_backend::irq::{DoorbellAcknowledge,RadioDoorbell,WifiIrqLine,WifiIrqPart,service_wifi_line,};"));
}

#[test]
fn invalid_models_and_configurations_are_refused() {
    let source = read(&format!("{APP}/execution.toml"));
    let model = model();
    let facts = facts();
    let hardware = hardware(&facts);
    let full = model.configurations().unwrap().pop().unwrap();
    let mut untraced = full.clone();
    untraced.instrumentation.clear();
    assert!(
        model
            .resolve(&untraced, &hardware)
            .unwrap_err()
            .to_string()
            .contains("instrumentation")
    );
    let mut unknown = full.clone();
    unknown.features.insert("invented".into());
    assert!(model.resolve(&unknown, &hardware).is_err());
    assert!(Model::parse(&source.replace("line = 40", "lien = 40")).is_err());
    for (label, broken) in [
        (
            "source missing from register model",
            source.replace("CPU_INTR_FROM_CPU_2", "NO_SUCH_SOURCE"),
        ),
        (
            "source reused",
            source.replace("SYSTIMER_TARGET1", "SYSTIMER_TARGET0"),
        ),
        (
            "unknown feature",
            source.replace("all = [\"wifi\"]", "all = [\"radio-started\"]"),
        ),
        (
            "unknown runtime state",
            source.replace("none = [\"radio-started\"]", "none = [\"wifi\"]"),
        ),
        ("wrong SWI", source.replace("swi = 2", "swi = 1")),
        (
            "wrong chip",
            source.replace("chip = \"esp32s31\"", "chip = \"esp32c5\""),
        ),
        (
            "impossible predicate",
            source.replace("all = [\"wifi\"]", "any = [\"wifi\"], none = [\"wifi\"]"),
        ),
        (
            "unknown context",
            source.replace("context = \"radio_executor\"", "context = \"absent\""),
        ),
        (
            "foreign peer direction",
            source.replace("executor = \"hart1\"", "executor = \"application\""),
        ),
        ("duplicate line", source.replace("line = 41", "line = 40")),
        ("invalid level", source.replace("level = 4", "level = 99")),
    ] {
        let parsed = Model::parse(&broken).unwrap();
        assert!(parsed.resolve(&full, &hardware).is_err(), "{label}");
    }
    let resolved = model.resolve(&full, &hardware).unwrap();
    assert!(
        resolved
            .validate_runtime(&BTreeSet::from(["network-stopping".into()]))
            .is_err()
    );
    let plain = model.resolve(&Configuration::default(), &hardware).unwrap();
    assert!(
        plain
            .validate_runtime(&BTreeSet::from(["radio-started".into()]))
            .is_err()
    );
    // A real but incorrect source remains structurally valid. The typed part
    // comparison, rather than existence/range validation, must catch it.
    let wrong = Model::parse(&source.replace("MODEM_WIFI_MAC\"", "MODEM_WIFI_MAC_NMI\""))
        .unwrap()
        .resolve(&full, &hardware)
        .unwrap();
    let wifi = wrong
        .entries
        .iter()
        .find(|e| e.declaration.name == "wifi")
        .unwrap();
    assert_ne!(
        wifi.source_ids,
        <WifiIrqPart<()> as InterruptSources>::SOURCES
    );
}
