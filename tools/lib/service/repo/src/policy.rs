//! Which package may depend on which: the one statement of the layer,
//! platform and role rules ([layer dependencies](../../../docs/architecture.md#layer-dependencies)).
//!
//! - A production package's normal and build path dependencies are
//!   production packages of a layer its own may use, never the facade, and
//!   of a compatible platform; a build dependency runs on the host.
//! - Only adapters, compositions and the facade bind an executor
//!   (`embassy-executor`) or the time driver (`embassy-time`,
//!   `oer-time-embassy`); the time-driver rule covers dev dependencies too.
//! - A verdict package never depends on a report package, and an
//!   observation package never on HIL orchestration or stand operation.
//! - A host package depends only on its own host layer and the layers below
//!   it ([`HostLayer`]): entry, orchestration, execution, verification,
//!   stand, build, foundation.

use crate::{
    Model,
    classification::{
        Classification, Evidence, Hil, HostApp, HostBoundary, HostLayer, Layer, Platform, Scope,
    },
    manifest::{Dependency, Kind},
};

/// Executor crates. Lower layers expose futures that any executor may poll.
pub const EXECUTORS: &[&str] = &["embassy-executor"];

/// Crates that read or wait on the image's one time driver: the driver
/// interface and its `oer-time` binding. Lower layers, runtimes included,
/// take time through the `oer-time` clock and timer ports; their tests use
/// virtual time, so the rule covers dev dependencies too.
pub const TIME_DRIVERS: &[&str] = &["embassy-time", "oer-time-embassy"];

/// The third-party format libraries each with the packages that may depend
/// on it through a normal or build dependency: one owner reads the format
/// and exposes only its own types, so no other package can name, and leak,
/// the library's types. `object` has a second owner, `blobray-artifacts`:
/// Blobray's metered, streaming reading of archive members and sections
/// under its work budget, the artifact identities it keeps for evidence and
/// its writing of ELF objects are not the whole-file view `oer-elf` gives
/// (<https://github.com/the predecessor repository/issues/334#issuecomment->).
pub const EXTERNAL_OWNERS: &[(&str, &[&str])] = &[
    ("object", &["oer-elf", "blobray-artifacts"]),
    ("gimli", &["oer-elf"]),
    ("addr2line", &["oer-elf"]),
    ("rv-asm", &["oer-riscv-decode"]),
];

/// Whether packages of `layer` may bind an executor or the time driver.
pub fn binds_runtime(layer: Layer) -> bool {
    matches!(layer, Layer::Adapter | Layer::Composition | Layer::Facade)
}

/// Responsibilities are not a single stack: an adapter may bind a service or
/// runtime interface to an executor, while a runtime may use an adapter for a
/// lower executor contract. Hardware owns chip resources and wire codecs; a
/// role composes portable role protocols with that hardware. Services declare
/// executor-free ports and never depend on the adapters that bind them. None
/// may acquire the final composition or public facade above them.
pub fn layer_allows(source: Layer, target: Layer) -> bool {
    use Layer::*;
    match source {
        Contract | Protocol => matches!(target, Contract | Protocol),
        Hardware => matches!(target, Contract | Protocol | Hardware),
        Role => matches!(target, Contract | Protocol | Hardware | Role),
        Service => matches!(target, Contract | Protocol | Service),
        Adapter | Runtime => matches!(
            target,
            Contract | Protocol | Hardware | Role | Adapter | Runtime | Service
        ),
        Composition | Facade => target != Facade,
        _ => false,
    }
}

/// Whether code of `source` may use code of `target` on its platform;
/// `family` names a chip's family. A build dependency runs on the host.
pub fn platform_allows(
    source: &Classification,
    target: &Platform,
    kind: Kind,
    family: &dyn Fn(&str) -> Option<String>,
) -> bool {
    if kind == Kind::Build {
        return matches!(target, Platform::Host | Platform::Portable);
    }
    match (&source.platform, target) {
        (_, Platform::Portable) => true,
        (Platform::Chip(source), Platform::Chip(target)) => source == target,
        (Platform::Host, Platform::Host) => true,
        // Family code is shared within its family only.
        (Platform::Family(source), Platform::Family(target)) => source == target,
        (Platform::Chip(chip), Platform::Family(target)) => {
            family(chip).is_some_and(|own| own == *target)
        }
        _ => source.layer == Layer::Facade,
    }
}

/// Whether a package with evidence role `source` may depend on one with
/// role `target`: a verdict never depends on a report, so report code can
/// neither decide a verdict nor enter a shard's sources.
pub fn evidence_edge_allowed(source: Option<Evidence>, target: Option<Evidence>) -> bool {
    !(source == Some(Evidence::Verdict) && target == Some(Evidence::Report))
}

/// Whether a package with HIL role `source` may depend on one with role
/// `target`: observation code reaches neither orchestration nor stand
/// operation, so neither can change what a run observes through it.
pub fn hil_edge_allowed(source: Option<Hil>, target: Option<Hil>) -> bool {
    !(source == Some(Hil::Observation)
        && matches!(target, Some(Hil::Orchestration | Hil::Operation)))
}

/// Whether a package of host layer `source` may depend on one of host layer
/// `target`: only on its own layer and those below it. A package without a
/// host layer (production code, a portable helper) constrains nothing.
pub fn host_edge_allowed(source: Option<HostLayer>, target: Option<HostLayer>) -> bool {
    match (source, target) {
        (Some(source), Some(target)) => target >= source,
        _ => true,
    }
}

/// Whether an application can link this package of another owner. Shared
/// libraries have explicit consumers; private application code never crosses
/// the boundary. Host layers still constrain edges within these sets.
pub fn application_edge_allowed(source: HostApp, target: HostApp, boundary: HostBoundary) -> bool {
    use HostApp::*;
    if source == target {
        return true;
    }
    if boundary == HostBoundary::Application {
        return false;
    }
    let permitted = match source {
        Gate => target == Foundation,
        Foundation | Formats => matches!(target, Foundation | Formats),
        Fw => matches!(target, Foundation | Devices | Images | Analysis | Formats),
        Devices => matches!(target, Foundation | Formats),
        Stand => matches!(target, Foundation | Devices | Formats),
        Hil => matches!(
            target,
            Foundation | Devices | Images | Analysis | Formats | Stand | Verification
        ),
        Verification => matches!(
            target,
            Foundation | Images | Analysis | Formats | Blobray | Registers
        ),
        Blobray | Analysis => matches!(target, Foundation | Analysis | Formats),
        Registers => matches!(target, Foundation | Analysis | Formats | Verification),
        Images => matches!(target, Foundation | Analysis | Formats),
    };
    permitted && (target != Formats || boundary == HostBoundary::Format)
}

/// The local packages each package of the shared RISC-V layer (host
/// application `analysis`) and of Blobray may depend on through a normal or
/// build dependency: the one table [`tools/riscv`](../../riscv/README.md) and
/// [Blobray's architecture](../../blobray/docs/design/architecture.md#crates-and-modules)
/// describe. The `oer-riscv-*` crates are Blobray's ISA layer: its
/// application may use the ISA-neutral model, the function analysis and the
/// static image view, and its command line selects the concrete decoder.
/// Dependencies on other applications' shared libraries follow
/// [`application_edge_allowed`].
pub const COMPONENTS: &[(&str, &[&str])] = &[
    ("oer-elf", &[]),
    ("oer-riscv-model", &[]),
    ("oer-riscv-decode", &[]),
    (
        "oer-riscv-lift",
        &["oer-elf", "oer-riscv-decode", "oer-riscv-model"],
    ),
    ("oer-riscv-analysis", &["oer-riscv-model"]),
    ("oer-riscv-program", &["oer-elf", "oer-riscv-model"]),
    (
        "oer-riscv-stack",
        &[
            "oer-elf",
            "oer-riscv-analysis",
            "oer-riscv-decode",
            "oer-riscv-lift",
            "oer-riscv-model",
            "oer-riscv-program",
        ],
    ),
    ("blobray-domain", &["oer-riscv-model"]),
    (
        "blobray-artifacts",
        &[
            "blobray-domain",
            "oer-elf",
            "oer-riscv-model",
            "oer-riscv-program",
        ],
    ),
    (
        "blobray-analysis",
        &["blobray-domain", "oer-riscv-analysis", "oer-riscv-model"],
    ),
    (
        "blobray-backend-riscv",
        &[
            "blobray-domain",
            "oer-riscv-decode",
            "oer-riscv-lift",
            "oer-riscv-model",
        ],
    ),
    (
        "blobray-verification",
        &["blobray-domain", "oer-riscv-model"],
    ),
    (
        "blobray-application",
        &[
            "blobray-analysis",
            "blobray-artifacts",
            "blobray-domain",
            "blobray-verification",
            "oer-riscv-analysis",
            "oer-riscv-model",
            "oer-riscv-program",
        ],
    ),
    (
        "blobray-linker",
        &["blobray-application", "blobray-domain", "oer-riscv-model"],
    ),
    (
        "blobray-cli",
        &[
            "blobray-application",
            "blobray-backend-riscv",
            "blobray-domain",
            "oer-riscv-lift",
            "oer-riscv-model",
        ],
    ),
];

/// Whether `app` is one whose packages [`COMPONENTS`] lists.
fn has_components(app: Option<HostApp>) -> bool {
    matches!(app, Some(HostApp::Analysis | HostApp::Blobray))
}

/// Every dependency of the model that breaks a rule, as
/// `<manifest>: <problem>`. Unclassified packages are the classification
/// check's; their edges are skipped.
pub fn check(model: &Model) -> Vec<String> {
    let family = |chip: &str| model.chips.family_of(chip).map(str::to_owned);
    let mut problems = Vec::new();
    for package in model.packages() {
        let Ok(source) = model.classification(package) else {
            continue;
        };
        let mut problem = |message: String| {
            problems.push(format!("{}: {message}", package.manifest));
        };
        let components = COMPONENTS
            .iter()
            .find(|(name, _)| *name == package.name)
            .map(|(_, allowed)| *allowed);
        if has_components(source.host_app) && components.is_none() {
            problem(format!(
                "{} package {} has no row in the component table (oer_repo::policy::COMPONENTS)",
                source.host_app.map_or("?", HostApp::name),
                package.name
            ));
        }
        for dependency in &package.dependencies {
            let target = model
                .target(dependency)
                .map(|target| (target, model.classification(target)));
            if let Some((target, Ok(class))) = target {
                if !evidence_edge_allowed(source.evidence, class.evidence) {
                    problem(format!(
                        "verdict package {} depends on report package {}",
                        package.name, target.name
                    ));
                }
                if let (Some(source_app), Some(target_app), Some(boundary)) =
                    (source.host_app, class.host_app, class.host_boundary)
                    && dependency.kind != Kind::Development
                    && !application_edge_allowed(source_app, target_app, boundary)
                {
                    problem(format!(
                        "host application {} package {} depends on {} {} package {}; applications link only their permitted shared libraries and formats",
                        source_app.name(),
                        package.name,
                        target_app.name(),
                        boundary.name(),
                        target.name
                    ));
                }
                if let Some(allowed) = components
                    && has_components(class.host_app)
                    && dependency.kind != Kind::Development
                    && !allowed.contains(&target.name.as_str())
                {
                    problem(format!(
                        "{} depends on {}, which its row of the component table (oer_repo::policy::COMPONENTS) does not allow",
                        package.name, target.name
                    ));
                }
                if !host_edge_allowed(source.host_layer, class.host_layer) {
                    problem(format!(
                        "{} package {} depends on {} package {}; a host package uses only its own host layer and those below it",
                        source.host_layer.map_or("?", HostLayer::name),
                        package.name,
                        class.host_layer.map_or("?", HostLayer::name),
                        target.name
                    ));
                }
                if !hil_edge_allowed(source.hil, class.hil) {
                    problem(format!(
                        "observation package {} depends on {} package {}",
                        package.name,
                        match class.hil {
                            Some(Hil::Orchestration) => "orchestration",
                            _ => "stand operation",
                        },
                        target.name
                    ));
                }
            }
            if dependency.kind != Kind::Development
                && let Some((_, owners)) = EXTERNAL_OWNERS
                    .iter()
                    .find(|(name, _)| *name == dependency.package)
                && !owners.contains(&package.name.as_str())
            {
                problem(format!(
                    "{} depends on {}, which only {} may depend on (oer_repo::policy::EXTERNAL_OWNERS); use its owner's interface",
                    package.name,
                    dependency.package,
                    owners.join(", ")
                ));
            }
            if source.scope != Scope::Production {
                continue;
            }
            if TIME_DRIVERS.contains(&dependency.package.as_str()) && !binds_runtime(source.layer) {
                problem(format!(
                    "{} package {} depends on time driver {}; only adapters, compositions and the facade bind it, and lower layers take time through oer-time",
                    source.layer, package.name, dependency.package
                ));
            }
            if dependency.kind == Kind::Development {
                continue;
            }
            if EXECUTORS.contains(&dependency.package.as_str()) && !binds_runtime(source.layer) {
                problem(format!(
                    "{} package {} depends on executor {}; only adapters and compositions bind an executor",
                    source.layer, package.name, dependency.package
                ));
            }
            production_edge(model, source, package, dependency, &family, &mut problem);
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

/// The rules of one normal or build path dependency of a production package.
fn production_edge(
    model: &Model,
    source: &Classification,
    package: &crate::Package,
    dependency: &Dependency,
    family: &dyn Fn(&str) -> Option<String>,
    problem: &mut dyn FnMut(String),
) {
    let Some(path) = &dependency.path else {
        return;
    };
    let target = model.package_at(path);
    let Some(class) = target
        .and_then(|target| model.classification(target).ok())
        .filter(|class| class.scope == Scope::Production)
    else {
        problem(format!(
            "production package {} depends on non-production package {}",
            package.name, dependency.package
        ));
        return;
    };
    if class.layer == Layer::Facade {
        problem(format!(
            "internal package {} depends on public facade {}",
            package.name, dependency.package
        ));
    } else if !layer_allows(source.layer, class.layer) {
        problem(format!(
            "forbidden architecture edge {} -> {}: {} depends on {}",
            source.layer, class.layer, package.name, dependency.package
        ));
    }
    if !platform_allows(source, &class.platform, dependency.kind, family) {
        problem(format!(
            "incompatible platform edge {:?} -> {:?}: {} depends on {}",
            source.platform, class.platform, package.name, dependency.package
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Model, files::Repo, testing::tree};

    const CHIP: (&str, &str) = (
        "platform/chip-a/chip.toml",
        "schema = 1\nid = \"chip-a\"\nfamily = \"espressif\"\nrust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\nespflash-chip = \"chip-a\"\nrevisions = [\"rev0\"]\n[properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = true\ncores = 2\n",
    );

    fn manifest(name: &str, class: &str, dependencies: &str) -> String {
        format!(
            "[package]\nname = \"{name}\"\n{dependencies}\n[package.metadata.open-radio]\n{class}\n"
        )
    }

    fn problems(files: &[(&str, String)]) -> Vec<String> {
        let mut all: Vec<(&str, &str)> = vec![CHIP];
        all.extend(files.iter().map(|(path, text)| (*path, text.as_str())));
        let dir = tree(&all);
        check(&Model::load(&Repo::from_dir(dir.path()).unwrap()).unwrap())
    }

    fn edge(source: &str, target: &str, section: &str) -> Vec<String> {
        problems(&[
            (
                "s/Cargo.toml",
                manifest(
                    "oer-s",
                    source,
                    &format!("[{section}]\noer-t = {{ path = \"../t\" }}"),
                ),
            ),
            ("t/Cargo.toml", manifest("oer-t", target, "")),
        ])
    }

    const PORTABLE: &str = "platform = \"portable\"";

    #[test]
    fn layers_follow_the_dependency_table() {
        let layer = |layer: &str| format!("layer = \"{layer}\"\n{PORTABLE}");
        assert!(edge(&layer("service"), &layer("protocol"), "dependencies").is_empty());
        assert_eq!(
            edge(&layer("service"), &layer("adapter"), "dependencies"),
            [
                "s/Cargo.toml: forbidden architecture edge service -> adapter: oer-s depends on oer-t"
            ]
        );
        // Optional and build edges count; dev edges may compose anything.
        assert_eq!(
            edge(&layer("protocol"), &layer("hardware"), "build-dependencies").len(),
            1
        );
        assert!(edge(&layer("protocol"), &layer("hardware"), "dev-dependencies").is_empty());
        assert_eq!(
            edge(&layer("composition"), &layer("facade"), "dependencies"),
            ["s/Cargo.toml: internal package oer-s depends on public facade oer-t"]
        );
        assert_eq!(
            edge(
                &layer("role"),
                "layer = \"tool\"\nplatform = \"host\"",
                "dependencies"
            ),
            ["s/Cargo.toml: production package oer-s depends on non-production package oer-t"]
        );
    }

    #[test]
    fn platforms_stay_within_their_chip_and_family() {
        let class = |platform: &str| format!("layer = \"hardware\"\n{platform}");
        let chip = class("platform = \"chip\"\nchip = \"chip-a\"");
        let family = class("platform = \"family\"\nfamily = \"espressif\"");
        let other = class("platform = \"family\"\nfamily = \"other\"");
        assert!(edge(&chip, &family, "dependencies").is_empty());
        assert_eq!(edge(&family, &chip, "dependencies").len(), 1);
        assert_eq!(edge(&chip, &other, "dependencies").len(), 1);
        assert_eq!(edge(&class(PORTABLE), &family, "dependencies").len(), 1);
        // A build script runs on the host.
        let message = edge(&chip, &chip, "build-dependencies");
        assert_eq!(message.len(), 1);
        assert!(
            message[0].contains("incompatible platform edge"),
            "{message:?}"
        );
    }

    #[test]
    fn executors_and_the_time_driver_belong_to_bindings() {
        let service = format!("layer = \"service\"\n{PORTABLE}");
        let found = problems(&[(
            "s/Cargo.toml",
            manifest(
                "oer-s",
                &service,
                "[dependencies]\nexec = { package = \"embassy-executor\", version = \"1\" }\n[dev-dependencies]\nembassy-time = \"1\"",
            ),
        )]);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("depends on executor embassy-executor"));
        assert!(found[1].contains("depends on time driver embassy-time"));
        let adapter = format!("layer = \"adapter\"\n{PORTABLE}");
        assert!(
            problems(&[(
                "s/Cargo.toml",
                manifest(
                    "oer-s",
                    &adapter,
                    "[dependencies]\nembassy-executor = \"1\"\nembassy-time = \"1\""
                ),
            )])
            .is_empty()
        );
    }

    #[test]
    fn role_edges_point_away_from_reports_orchestration_and_operation() {
        let hil = |role: &str| {
            format!(
                "layer = \"hil\"\nplatform = \"host\"\nhil = \"{role}\"\nhost-layer = \"execution\""
            )
        };
        assert_eq!(
            edge(&hil("observation"), &hil("operation"), "dependencies"),
            ["s/Cargo.toml: observation package oer-s depends on stand operation package oer-t"]
        );
        assert_eq!(
            edge(
                &hil("observation"),
                &hil("orchestration"),
                "dev-dependencies"
            ),
            ["s/Cargo.toml: observation package oer-s depends on orchestration package oer-t"]
        );
        assert!(edge(&hil("orchestration"), &hil("operation"), "dependencies").is_empty());
        assert!(edge(&hil("operation"), &hil("observation"), "dependencies").is_empty());
        let evidence = |role: &str| {
            format!(
                "layer = \"verification\"\nplatform = \"host\"\nevidence = \"{role}\"\nhost-layer = \"verification\""
            )
        };
        assert_eq!(
            edge(&evidence("verdict"), &evidence("report"), "dependencies").len(),
            1
        );
        assert!(edge(&evidence("report"), &evidence("verdict"), "dependencies").is_empty());
    }

    #[test]
    fn host_packages_depend_only_down_their_layers() {
        let host = |layer: &str| {
            format!("layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"{layer}\"")
        };
        assert!(edge(&host("entry"), &host("orchestration"), "dependencies").is_empty());
        assert!(edge(&host("entry"), &host("entry"), "dependencies").is_empty());
        assert!(edge(&host("stand"), &host("build"), "dependencies").is_empty());
        for (source, target) in [
            ("stand", "execution"),
            ("build", "stand"),
            ("verification", "execution"),
            ("foundation", "build"),
            ("orchestration", "entry"),
        ] {
            assert_eq!(
                edge(&host(source), &host(target), "dependencies"),
                [format!(
                    "s/Cargo.toml: {source} package oer-s depends on {target} package oer-t; a host package uses only its own host layer and those below it"
                )],
                "{source} -> {target}"
            );
        }
        // Production code has no host layer and constrains nothing.
        assert!(
            edge(
                &host("foundation"),
                &format!("layer = \"role\"\n{PORTABLE}"),
                "dependencies"
            )
            .is_empty()
        );
    }

    #[test]
    fn blobray_and_the_riscv_layer_depend_only_as_the_component_table_allows() {
        let class = |app: &str| {
            format!(
                "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"\nhost-app = \"{app}\"\nhost-boundary = \"library\""
            )
        };
        let pair = |source: (&str, &str), target: (&str, &str), section: &str| {
            problems(&[
                (
                    "s/Cargo.toml",
                    manifest(
                        source.0,
                        &class(source.1),
                        &format!("[{section}]\n{} = {{ path = \"../t\" }}", target.0),
                    ),
                ),
                ("t/Cargo.toml", manifest(target.0, &class(target.1), "")),
            ])
        };
        let application = ("blobray-application", "blobray");
        let domain = ("blobray-domain", "blobray");
        let program = ("oer-riscv-program", "analysis");
        // Rows allow exactly their listed edges, across the two applications.
        assert!(pair(application, domain, "dependencies").is_empty());
        assert!(pair(application, program, "build-dependencies").is_empty());
        assert_eq!(
            pair(domain, application, "dependencies"),
            [
                "s/Cargo.toml: blobray-domain depends on blobray-application, which its row of the component table (oer_repo::policy::COMPONENTS) does not allow"
            ]
        );
        assert_eq!(
            pair(program, ("oer-riscv-analysis", "analysis"), "dependencies").len(),
            1
        );
        // Tests may use any package.
        assert!(pair(domain, application, "dev-dependencies").is_empty());
        // A package of either application must have a row.
        assert_eq!(
            pair(("blobray-extra", "blobray"), domain, "dependencies"),
            [
                "s/Cargo.toml: blobray package blobray-extra has no row in the component table (oer_repo::policy::COMPONENTS)"
            ]
        );
    }

    #[test]
    fn a_format_library_is_a_dependency_of_its_owners_only() {
        let host = "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"build\"";
        let depends = |name: &str, section: &str| {
            problems(&[(
                "s/Cargo.toml",
                manifest(name, host, &format!("[{section}]\nobject = \"=0.39.1\"")),
            )])
        };
        assert!(depends("oer-elf", "dependencies").is_empty());
        assert_eq!(
            depends("oer-riscv-program", "dependencies"),
            [
                "s/Cargo.toml: oer-riscv-program depends on object, which only oer-elf, blobray-artifacts may depend on (oer_repo::policy::EXTERNAL_OWNERS); use its owner's interface"
            ]
        );
        assert_eq!(depends("oer-riscv-program", "build-dependencies").len(), 1);
        // Blobray's streaming reader owns it too; the image encoder reads
        // ELF through oer-elf.
        assert!(depends("blobray-artifacts", "dependencies").is_empty());
        assert_eq!(depends("oer-image-encode", "dependencies").len(), 1);
        // Tests may write fixtures with it.
        assert!(depends("oer-riscv-program", "dev-dependencies").is_empty());
    }

    #[test]
    fn applications_link_only_their_declared_shared_boundaries() {
        let host = |app: &str, boundary: &str| {
            format!(
                "layer = \"tool\"\nplatform = \"host\"\nhost-layer = \"entry\"\nhost-app = \"{app}\"\nhost-boundary = \"{boundary}\""
            )
        };
        // The synthetic Blobray package has no component-table row; that rule
        // has its own test.
        let edge = |source: &str, target: &str, kind: &str| -> Vec<String> {
            edge(source, target, kind)
                .into_iter()
                .filter(|problem| !problem.contains("component table"))
                .collect()
        };
        for kind in ["dependencies", "build-dependencies"] {
            for (source, target, boundary) in [
                ("fw", "stand", "library"),
                ("gate", "images", "library"),
                ("hil", "stand", "application"),
                ("verification", "blobray", "application"),
            ] {
                let problems = edge(&host(source, "application"), &host(target, boundary), kind);
                assert_eq!(problems.len(), 1, "{source} -> {target}: {problems:?}");
                assert!(problems[0].contains("applications link only"));
            }
            for (source, target, boundary) in [
                ("fw", "images", "library"),
                ("hil", "stand", "library"),
                ("verification", "blobray", "library"),
                ("formats", "foundation", "library"),
                ("gate", "foundation", "library"),
            ] {
                assert!(
                    edge(&host(source, "application"), &host(target, boundary), kind).is_empty()
                );
            }
        }
        // Tests may compose applications; normal/build dependencies cannot.
        assert!(
            edge(
                &host("fw", "application"),
                &host("stand", "application"),
                "dev-dependencies"
            )
            .is_empty()
        );
    }
}
