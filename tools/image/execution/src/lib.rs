//! Host-only execution model. The firmware does not consume this model yet.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use oer_chips_family_facts::InterruptContract;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("execution model: {0}")]
    Invalid(String),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

fn require(holds: bool, message: impl FnOnce() -> String) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Error::Invalid(message()))
    }
}

/// Feature predicates and runtime predicates use different name domains.
/// `any = []` imposes no additional requirement.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    #[serde(default)]
    pub all: BTreeSet<String>,
    #[serde(default)]
    pub any: BTreeSet<String>,
    #[serde(default)]
    pub none: BTreeSet<String>,
}

impl Predicate {
    pub fn matches(&self, selected: &BTreeSet<String>) -> bool {
        self.all.is_subset(selected)
            && (self.any.is_empty() || !self.any.is_disjoint(selected))
            && self.none.is_disjoint(selected)
    }

    fn validate(&self, names: &BTreeSet<String>, owner: &str) -> Result<()> {
        for name in self.all.iter().chain(&self.any).chain(&self.none) {
            require(names.contains(name), || {
                format!("{owner}: unknown condition {name}")
            })?;
        }
        require(self.all.is_disjoint(&self.none), || {
            format!("{owner}: contradictory condition")
        })?;
        require(
            self.any.is_empty() || !self.any.is_subset(&self.none),
            || format!("{owner}: unsatisfiable any condition"),
        )
    }
}

/// The raw conditional wrapper is removed by resolution. Deserializing the
/// payload separately lets every payload reject unknown fields as well.
#[derive(Clone, Debug)]
pub struct Choice<T> {
    pub when: Predicate,
    pub value: T,
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Choice<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let mut fields = toml::Table::deserialize(deserializer)?;
        let when = fields
            .remove("when")
            .map(toml::Value::try_into)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        let value = toml::Value::Table(fields)
            .try_into()
            .map_err(serde::de::Error::custom)?;
        Ok(Self { when, value })
    }
}

macro_rules! records {
    ($(pub struct $name:ident { $($(#[$attr:meta])* pub $field:ident: $ty:ty),* $(,)? })*) => {$(
        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(deny_unknown_fields, rename_all = "kebab-case")]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
    )*};
}

records! {
    pub struct Requirement { pub when: Predicate, pub instrumentation: BTreeSet<String> }
    pub struct State { pub name: String, #[serde(default)] pub requires: BTreeSet<String> }
    pub struct RuntimeState { pub available: bool, pub requires: BTreeSet<String> }
    pub struct Context { pub name: String, pub hart: u8, pub level: u8, pub kind: ContextKind }
    pub struct Entry {
        pub name: String, pub context: String, pub line: u8,
        pub part: String, pub handler: String, pub class: Class,
        #[serde(default)] pub reason: String,
        pub sources: Vec<String>
    }
    pub struct Executor { pub name: String, pub context: String, pub entry: String, pub swi: u8 }
    pub struct Software { pub name: String, pub entry: String, pub swi: u8 }
    pub struct TaskSet { pub name: String, pub context: String, pub observer: u8 }
    pub struct TaskRoot { pub name: String, pub context: String, pub members: Vec<String> }
    pub struct Task { pub name: String, pub context: String }
    pub struct Signal {
        pub name: String, pub target: String, pub raisers: Vec<Raiser>,
        #[serde(default)] pub active_when: Predicate
    }
    pub struct PeerNotify { pub name: String, pub signal: String }
    pub struct Timer {
        pub name: String, pub inventory: String, pub entry: String, pub target: String,
        #[serde(default)] pub active_when: Predicate
    }
    pub struct HartStart { pub step: String, pub hart: u8 }
    pub struct NumberUse { pub id: String, pub kind: NumberKind, pub owner: String }
    pub struct Bound { pub kind: BoundKind, pub detail: String }
    pub struct Wait {
        pub id: String, pub waiter: Vec<String>, pub served_by: Vec<String>,
        pub service: String, pub held: String, pub bound: Bound,
        #[serde(default)] pub active_when: Predicate
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ContextKind {
    Thread,
    InterruptExecutor,
    Handler,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Hardware,
    Executor,
    ExemptExecutorLevel,
    ExemptWakerCall,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NumberKind {
    Swi,
    Hart,
    Observer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoundKind {
    None,
    BootDeadline,
    OperationDeadline,
    ConditionalDeadlines,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", deny_unknown_fields, rename_all = "kebab-case")]
pub enum Raiser {
    Handler { entry: String },
    ExecutorLevel { entry: String },
    Task { executor: String },
    OwnTask { executor: String },
    PeerHart { executor: String },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Model {
    pub schema: u32,
    pub chip: String,
    pub features: BTreeSet<String>,
    pub instrumentation: BTreeSet<String>,
    pub external: BTreeSet<String>,
    #[serde(default)]
    pub requirement: Vec<Requirement>,
    #[serde(default)]
    pub state: Vec<Choice<State>>,
    pub context: Vec<Choice<Context>>,
    pub entry: Vec<Choice<Entry>>,
    pub executor: Vec<Choice<Executor>>,
    #[serde(default)]
    pub software: Vec<Choice<Software>>,
    pub task_set: Vec<Choice<TaskSet>>,
    pub task_root: Vec<Choice<TaskRoot>>,
    pub task: Vec<Choice<Task>>,
    pub signal: Vec<Choice<Signal>>,
    pub peer_notify: Vec<Choice<PeerNotify>>,
    pub timer: Vec<Choice<Timer>>,
    pub hart_start: Vec<Choice<HartStart>>,
    pub number_use: Vec<Choice<NumberUse>>,
    pub wait: Vec<Choice<Wait>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Configuration {
    pub features: BTreeSet<String>,
    pub instrumentation: BTreeSet<String>,
}

/// Hardware input is supplied from chip facts and the register model. SWI
/// identities come from the chip's typed parts, never from execution.toml.
pub struct Hardware<'a> {
    pub chip: &'a str,
    pub interrupts: &'a InterruptContract,
    pub sources: BTreeMap<String, u16>,
    pub software: BTreeMap<u8, u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedEntry {
    #[serde(flatten)]
    pub declaration: Entry,
    pub hart: u8,
    pub level: u8,
    pub source_ids: Vec<u16>,
}

/// There is no `when` field in this type or its records. Runtime predicates
/// remain; state availability has already been reduced to a boolean.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Resolved {
    pub chip: String,
    pub configuration: Configuration,
    pub states: BTreeMap<String, RuntimeState>,
    pub harts: BTreeSet<u8>,
    pub contexts: Vec<Context>,
    pub entries: Vec<ResolvedEntry>,
    pub executors: Vec<Executor>,
    pub software: Vec<Software>,
    pub task_sets: Vec<TaskSet>,
    pub task_roots: Vec<TaskRoot>,
    pub tasks: Vec<Task>,
    pub signals: Vec<Signal>,
    pub peer_notifies: Vec<PeerNotify>,
    pub timers: Vec<Timer>,
    pub hart_start: Vec<HartStart>,
    pub number_uses: Vec<NumberUse>,
    pub waits: Vec<Wait>,
}

fn select<T: Clone>(
    choices: &[Choice<T>],
    config: &Configuration,
    known: &BTreeSet<String>,
) -> Result<Vec<T>> {
    let mut chosen = Vec::new();
    for choice in choices {
        choice.when.validate(known, "when")?;
        if choice.when.matches(&config.features) {
            chosen.push(choice.value.clone());
        }
    }
    Ok(chosen)
}

fn index<'a, T>(
    rows: &'a [T],
    name: impl Fn(&'a T) -> &'a str,
    kind: &str,
) -> Result<BTreeMap<&'a str, &'a T>> {
    let mut indexed = BTreeMap::new();
    for row in rows {
        let key = name(row);
        require(
            !key.trim().is_empty() && indexed.insert(key, row).is_none(),
            || format!("empty or duplicate {kind}: {key}"),
        )?;
    }
    Ok(indexed)
}

fn lookup<'a, T>(indexed: &BTreeMap<&str, &'a T>, key: &str, kind: &str) -> Result<&'a T> {
    indexed
        .get(key)
        .copied()
        .ok_or_else(|| Error::Invalid(format!("unknown {kind}: {key}")))
}

impl Model {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// Enumerate every subset of the declared features, adding only the
    /// instrumentation required by that subset. Optional instrumentation is
    /// accepted by resolve too; it does not select execution records.
    pub fn configurations(&self) -> Result<Vec<Configuration>> {
        require(self.features.len() <= 16, || {
            "too many feature dimensions to enumerate (maximum 16)".into()
        })?;
        let names: Vec<_> = self.features.iter().collect();
        let mut configs = Vec::new();
        for bits in 0..(1_usize << names.len()) {
            let features = names
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, name)| (*name).clone())
                .collect();
            let mut config = Configuration {
                features,
                ..Configuration::default()
            };
            for rule in &self.requirement {
                rule.when.validate(&self.features, "requirement")?;
                require(
                    rule.instrumentation.is_subset(&self.instrumentation),
                    || "unknown required instrumentation".into(),
                )?;
                if rule.when.matches(&config.features) {
                    config.instrumentation.extend(rule.instrumentation.clone());
                }
            }
            configs.push(config);
        }
        Ok(configs)
    }

    pub fn resolve(&self, config: &Configuration, hardware: &Hardware<'_>) -> Result<Resolved> {
        require(self.schema == 1, || {
            format!("unsupported schema {}", self.schema)
        })?;
        require(self.chip == hardware.chip, || {
            "model and hardware name different chips".into()
        })?;
        require(config.features.is_subset(&self.features), || {
            "unknown selected feature".into()
        })?;
        require(
            config.instrumentation.is_subset(&self.instrumentation),
            || "unknown instrumentation".into(),
        )?;
        for rule in &self.requirement {
            rule.when.validate(&self.features, "requirement")?;
            require(
                rule.instrumentation.is_subset(&self.instrumentation),
                || "unknown required instrumentation".into(),
            )?;
            require(
                !rule.when.matches(&config.features)
                    || rule.instrumentation.is_subset(&config.instrumentation),
                || "selected features require additional instrumentation".into(),
            )?;
        }
        let states = index(&self.state, |state| &state.value.name, "runtime state")?;
        let names = states
            .keys()
            .map(|s| (*s).to_owned())
            .collect::<BTreeSet<_>>();
        let states = states
            .into_iter()
            .map(|(name, state)| {
                state.when.validate(&self.features, "state availability")?;
                require(state.value.requires.is_subset(&names), || {
                    format!("{name}: unknown required runtime state")
                })?;
                Ok((
                    name.to_owned(),
                    RuntimeState {
                        available: state.when.matches(&config.features),
                        requires: state.value.requires.clone(),
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        macro_rules! selected {
            ($field:ident) => {
                select(&self.$field, config, &self.features)?
            };
        }
        let contexts: Vec<Context> = selected!(context);
        let context_index = index(&contexts, |row| &row.name, "context")?;
        for context in &contexts {
            require(
                hardware.interrupts.harts.contains(&u32::from(context.hart)),
                || format!("{}: unavailable hart", context.name),
            )?;
            let levels = hardware
                .interrupts
                .levels()
                .map_err(|e| Error::Invalid(e.to_string()))?;
            require(
                if context.kind == ContextKind::Thread {
                    context.level == 0
                } else {
                    context.level > 0 && u32::from(context.level) <= levels
                },
                || format!("{}: invalid execution level", context.name),
            )?;
        }
        let declarations: Vec<Entry> = selected!(entry);
        let mut entries = Vec::new();
        for declaration in declarations {
            let context = lookup(&context_index, &declaration.context, "entry context")?;
            require(context.kind != ContextKind::Thread, || {
                format!("{}: entry uses a thread context", declaration.name)
            })?;
            require(
                (hardware.interrupts.first_source_line..hardware.interrupts.lines)
                    .contains(&u32::from(declaration.line)),
                || format!("{}: invalid CPU line", declaration.name),
            )?;
            require(
                !declaration.part.is_empty()
                    && !declaration.handler.is_empty()
                    && !declaration.sources.is_empty(),
                || format!("{}: missing part, handler or sources", declaration.name),
            )?;
            let source_ids = declaration
                .sources
                .iter()
                .map(|name| {
                    hardware.sources.get(name).copied().ok_or_else(|| {
                        Error::Invalid(format!("{}: unknown source {name}", declaration.name))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            entries.push(ResolvedEntry {
                declaration,
                hart: context.hart,
                level: context.level,
                source_ids,
            });
        }
        let model = Resolved {
            chip: self.chip.clone(),
            configuration: config.clone(),
            states,
            harts: contexts.iter().map(|c| c.hart).collect(),
            contexts,
            entries,
            executors: selected!(executor),
            software: selected!(software),
            task_sets: selected!(task_set),
            task_roots: selected!(task_root),
            signals: selected!(signal),
            peer_notifies: selected!(peer_notify),
            tasks: selected!(task),
            timers: selected!(timer),
            hart_start: selected!(hart_start),
            number_uses: selected!(number_use),
            waits: selected!(wait),
        };
        model.validate(hardware, &self.external)?;
        Ok(model)
    }
}

impl Resolved {
    fn validate(&self, hardware: &Hardware<'_>, external: &BTreeSet<String>) -> Result<()> {
        let contexts = index(&self.contexts, |r| &r.name, "context")?;
        let entries = index(&self.entries, |r| &r.declaration.name, "entry")?;
        let executors = index(&self.executors, |r| &r.name, "executor")?;
        let software = index(&self.software, |r| &r.name, "software use")?;
        let task_sets = index(&self.task_sets, |r| &r.name, "task set")?;
        let _roots = index(&self.task_roots, |r| &r.name, "task root")?;
        let tasks = index(&self.tasks, |r| &r.name, "task role")?;
        let signals = index(&self.signals, |r| &r.name, "signal")?;
        let _peers = index(&self.peer_notifies, |r| &r.name, "peer notify")?;
        let _timers = index(&self.timers, |r| &r.name, "timer")?;
        let _starts = index(&self.hart_start, |r| &r.step, "hart start")?;
        let _numbers = index(&self.number_uses, |r| &r.id, "number use")?;
        let _waits = index(&self.waits, |r| &r.id, "wait")?;
        let state_names = self.states.keys().cloned().collect();
        for (name, state) in &self.states {
            require(
                !state.available
                    || state.requires.iter().all(|required| {
                        self.states
                            .get(required)
                            .is_some_and(|state| state.available)
                    }),
                || format!("{name}: required runtime state unavailable in this configuration"),
            )?;
        }
        let mut lines = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut last_lines = BTreeMap::new();
        let mut executor_levels = BTreeSet::new();
        for entry in &self.entries {
            require(lines.insert((entry.hart, entry.declaration.line)), || {
                "duplicate CPU line on one hart".into()
            })?;
            for source in &entry.source_ids {
                require(sources.insert(source), || {
                    format!("duplicate interrupt source {source}")
                })?;
            }
            if let Some(last) = last_lines.insert((entry.hart, entry.level), entry.declaration.line)
            {
                require(last < entry.declaration.line, || {
                    "equal-level entries violate highest-line arbitration order".into()
                })?;
            }
            if entry.declaration.class == Class::Executor {
                require(executor_levels.insert((entry.hart, entry.level)), || {
                    "two executor entries at one level on one hart".into()
                })?;
            }
        }
        for entry in &self.entries {
            let at_executor_level = executor_levels.contains(&(entry.hart, entry.level));
            match entry.declaration.class {
                Class::Hardware | Class::ExemptWakerCall => require(!at_executor_level, || {
                    format!("{}: hardware at executor level", entry.declaration.name)
                })?,
                Class::ExemptExecutorLevel => require(at_executor_level, || {
                    "executor-level exemption without executor".into()
                })?,
                Class::Executor => {}
            }
            if matches!(
                entry.declaration.class,
                Class::ExemptExecutorLevel | Class::ExemptWakerCall
            ) {
                require(!entry.declaration.reason.trim().is_empty(), || {
                    "exemption needs a reason".into()
                })?;
            }
        }
        let mut used_swi = BTreeSet::new();
        let mut execution_contexts = BTreeSet::new();
        let mut thread_harts = BTreeSet::new();
        for executor in &self.executors {
            let context = lookup(&contexts, &executor.context, "executor context")?;
            let entry = lookup(&entries, &executor.entry, "executor entry")?;
            require(execution_contexts.insert(executor.context.as_str()), || {
                "two executors claim one context".into()
            })?;
            require(used_swi.insert(executor.swi), || {
                "software interrupt claimed twice".into()
            })?;
            let source = hardware
                .software
                .get(&executor.swi)
                .ok_or_else(|| Error::Invalid("unknown software interrupt number".into()))?;
            require(
                entry.source_ids == [*source] && context.hart == entry.hart,
                || {
                    format!(
                        "{}: wake entry does not match executor SWI/hart",
                        executor.name
                    )
                },
            )?;
            match context.kind {
                ContextKind::Thread => {
                    require(
                        entry.declaration.class == Class::Hardware
                            && thread_harts.insert(context.hart),
                        || "thread executor needs its own hart and hardware wake entry".into(),
                    )?;
                }
                ContextKind::InterruptExecutor => require(
                    entry.declaration.class == Class::Executor
                        && entry.declaration.context == executor.context,
                    || "interrupt executor entry/context mismatch".into(),
                )?,
                ContextKind::Handler => {
                    return Err(Error::Invalid("executor uses handler context".into()));
                }
            }
        }
        for entry in &self.entries {
            if entry.declaration.class == Class::Executor {
                require(
                    self.executors
                        .iter()
                        .any(|x| x.entry == entry.declaration.name),
                    || "executor entry has no executor binding".into(),
                )?;
            }
        }
        for use_ in &self.software {
            let entry = lookup(&entries, &use_.entry, "software entry")?;
            let source = hardware
                .software
                .get(&use_.swi)
                .ok_or_else(|| Error::Invalid("unknown software interrupt number".into()))?;
            require(
                used_swi.insert(use_.swi) && entry.source_ids.contains(source),
                || "software use is duplicated or absent from its entry".into(),
            )?;
        }
        let mut observers = BTreeSet::new();
        for set in &self.task_sets {
            require(
                execution_contexts.contains(set.context.as_str()) && observers.insert(set.observer),
                || {
                    format!(
                        "{}: missing executor context or duplicate observer",
                        set.name
                    )
                },
            )?;
        }
        for root in &self.task_roots {
            require(
                execution_contexts.contains(root.context.as_str()) && !root.members.is_empty(),
                || format!("{}: root has no executor context or members", root.name),
            )?;
        }
        for task in &self.tasks {
            require(execution_contexts.contains(task.context.as_str()), || {
                format!("{}: task role has no executor context", task.name)
            })?;
        }
        for root in &self.task_roots {
            for member in &root.members {
                let task = lookup(&tasks, member, "root task role")?;
                require(task.context == root.context, || {
                    format!("{}: root task is on another context", root.name)
                })?;
            }
        }
        for signal in &self.signals {
            signal.active_when.validate(&state_names, &signal.name)?;
            let target = lookup(&executors, &signal.target, "signal target")?;
            let to = lookup(&contexts, &target.context, "target context")?;
            require(!signal.raisers.is_empty(), || {
                format!("{}: no raisers", signal.name)
            })?;
            for raiser in &signal.raisers {
                let valid = match raiser {
                    Raiser::Handler { entry } => {
                        let from = lookup(&entries, entry, "raiser entry")?;
                        from.hart == to.hart && from.level > to.level
                    }
                    Raiser::ExecutorLevel { entry } => {
                        let from = lookup(&entries, entry, "raiser entry")?;
                        from.hart == to.hart
                            && from.level == to.level
                            && from.declaration.class == Class::ExemptExecutorLevel
                    }
                    Raiser::Task { executor }
                    | Raiser::OwnTask { executor }
                    | Raiser::PeerHart { executor } => {
                        let from = lookup(&executors, executor, "raising executor")?;
                        let from_context = lookup(&contexts, &from.context, "raising context")?;
                        match raiser {
                            Raiser::Task { .. } => {
                                from_context.hart == to.hart && from_context.level < to.level
                            }
                            Raiser::OwnTask { .. } => executor == &signal.target,
                            Raiser::PeerHart { .. } => from_context.hart != to.hart,
                            _ => false,
                        }
                    }
                };
                require(valid, || {
                    format!("{}: invalid raiser direction", signal.name)
                })?;
            }
        }
        for peer in &self.peer_notifies {
            let _ = lookup(&signals, &peer.signal, "peer signal")?;
        }
        for timer in &self.timers {
            let _ = lookup(&entries, &timer.entry, "timer entry")?;
            let _ = lookup(&contexts, &timer.target, "timer target")?;
            timer.active_when.validate(&state_names, &timer.name)?;
        }
        for start in &self.hart_start {
            require(self.harts.contains(&start.hart), || {
                format!("{}: startup hart absent", start.step)
            })?;
        }
        for number in &self.number_uses {
            let _ = self.number_value(number)?;
        }
        for wait in &self.waits {
            wait.active_when.validate(&state_names, &wait.id)?;
            require(
                !wait.waiter.is_empty()
                    && !wait.served_by.is_empty()
                    && !wait.service.trim().is_empty()
                    && !wait.held.trim().is_empty()
                    && !wait.bound.detail.trim().is_empty(),
                || format!("{}: incomplete wait description", wait.id),
            )?;
            for endpoint in wait.waiter.iter().chain(&wait.served_by) {
                let valid = match endpoint.split_once(':') {
                    Some(("context", name)) => contexts.contains_key(name),
                    Some(("entry", name)) => entries.contains_key(name),
                    Some(("task", name)) => tasks.contains_key(name),
                    Some(("external", name)) => external.contains(name),
                    _ => false,
                };
                require(valid, || {
                    format!("{}: unknown wait endpoint {endpoint}", wait.id)
                })?;
            }
        }
        // Keep the indexes alive through validation; a new owner kind must be
        // added explicitly, not admitted as an untyped number.
        let _ = (software, task_sets);
        Ok(())
    }

    /// Value a currently handwritten source site must agree with. This reads
    /// only the resolved model; it does not inspect or rewrite Rust source.
    pub fn number_value(&self, use_: &NumberUse) -> Result<u8> {
        let (kind, name) = use_
            .owner
            .split_once(':')
            .ok_or_else(|| Error::Invalid("number owner needs a kind".into()))?;
        match (use_.kind, kind) {
            (NumberKind::Swi, "executor") => self
                .executors
                .iter()
                .find(|x| x.name == name)
                .map(|x| x.swi),
            (NumberKind::Swi, "software") => {
                self.software.iter().find(|x| x.name == name).map(|x| x.swi)
            }
            (NumberKind::Hart, "executor") => self
                .executors
                .iter()
                .find(|x| x.name == name)
                .and_then(|x| self.contexts.iter().find(|c| c.name == x.context))
                .map(|c| c.hart),
            (NumberKind::Observer, "task-set") => self
                .task_sets
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.observer),
            _ => None,
        }
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}: unknown or incompatible number owner {}",
                use_.id, use_.owner
            ))
        })
    }

    pub fn validate_runtime(&self, active: &BTreeSet<String>) -> Result<()> {
        require(
            active.iter().all(|name| {
                self.states
                    .get(name)
                    .is_some_and(|state| state.available && state.requires.is_subset(active))
            }),
            || "runtime snapshot names an unavailable state or misses its prerequisite".into(),
        )
    }

    /// Review candidates, not a deadlock verdict. A deadline may terminate an
    /// operation with a refusal and does not establish successful service.
    pub fn wait_risks(&self, active: &BTreeSet<String>) -> Result<Vec<&Wait>> {
        self.validate_runtime(active)?;
        Ok(self
            .waits
            .iter()
            .filter(|wait| {
                wait.active_when.matches(active)
                    && matches!(
                        wait.bound.kind,
                        BoundKind::None | BoundKind::ConditionalDeadlines
                    )
            })
            .collect())
    }
}
