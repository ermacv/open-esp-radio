use super::*;
use std::collections::BTreeSet;

/// A shard of `scenario` recording `sources`, without claims.
pub(crate) fn shard(scenario: &str, sources: Vec<crate::SourceDigest>) -> crate::Index {
    crate::Index {
        schema: crate::SCHEMA,
        command: crate::BLOBRAY.into(),
        target: "chip-a".into(),
        scenario: scenario.into(),
        inputs: Default::default(),
        sources,
        dependence: crate::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    }
}

#[test]
fn a_written_shard_reads_back_and_lists_by_name() {
    let directory = tempfile::tempdir().unwrap();
    let index = directory.path().join("index");
    assert!(names(&index).unwrap().is_empty());
    let digest = crate::SourceDigest {
        path: PathBuf::from("crates/phy"),
        sha256: "a".repeat(64),
    };
    let written = shard("rx-gain", vec![digest]);
    let file = write(&index, &written).unwrap();
    assert_eq!(file, index.join("rx-gain.json"));
    assert!(std::fs::read_to_string(&file).unwrap().ends_with("}\n"));
    assert_eq!(read(&index, "rx-gain"), Some(written.clone()));
    assert_eq!(read(&index, "absent"), None);
    std::fs::write(index.join("notes.txt"), "not a shard").unwrap();
    assert_eq!(names(&index).unwrap(), ["rx-gain"]);
    assert_eq!(shards(&index).unwrap(), [written]);
}

#[test]
fn a_host_stand_shard_is_accepted_and_an_unknown_producer_is_not() {
    let mut stand = shard(
        "ieee802154-host",
        vec![crate::SourceDigest {
            path: PathBuf::from("verification/chip-a/host/ieee802154"),
            sha256: "b".repeat(64),
        }],
    );
    stand.command = crate::HOST_STAND.into();
    stand.validate("chip-a").unwrap();
    stand.command = "hand-written".into();
    assert!(stand.validate("chip-a").is_err());
}

#[test]
fn a_shard_is_current_until_a_recorded_source_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::write(root.join("sources/lib.rs"), "one").unwrap();
    let shard = |scenario: &str| crate::Index {
        schema: crate::SCHEMA,
        command: crate::BLOBRAY.into(),
        target: "chip-a".into(),
        scenario: scenario.into(),
        inputs: Default::default(),
        sources: vec![crate::SourceDigest {
            sha256: crate::digest_directory(root, Path::new("sources")).unwrap(),
            path: PathBuf::from("sources"),
        }],
        dependence: crate::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    let current = shard("current");
    assert!(current.is_current(root));
    std::fs::write(root.join("sources/lib.rs"), "two").unwrap();
    assert!(!current.is_current(root));
}

#[test]
fn a_shard_tracking_executed_files_stays_current_when_another_file_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("crates/hal/src")).unwrap();
    std::fs::write(root.join("crates/hal/src/executed.rs"), "one").unwrap();
    std::fs::write(root.join("crates/hal/src/other.rs"), "one").unwrap();
    let executed = PathBuf::from("crates/hal/src/executed.rs");
    let shard = crate::Index {
        schema: crate::SCHEMA,
        command: crate::BLOBRAY.into(),
        target: "chip-a".into(),
        scenario: "leaf".into(),
        inputs: Default::default(),
        sources: vec![crate::SourceDigest {
            sha256: crate::digest_source(root, &executed).unwrap(),
            path: executed.clone(),
        }],
        dependence: crate::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged: vec![],
        functions: vec![],
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    std::fs::write(root.join("crates/hal/src/other.rs"), "two").unwrap();
    assert!(shard.is_current(root));
    std::fs::write(root.join(&executed), "two").unwrap();
    assert!(!shard.is_current(root));
}

#[test]
fn a_shard_of_the_previous_schema_is_rejected() {
    let text = r#"{"schema":10,"command":"vendor-scenario","target":"chip-a","scenario":"a",
        "inputs":{},"sources":[],"entries":[],"untriaged":[],"functions":[],"unobserved":[],
        "observed":[],"unprojected":[]}"#;
    assert!(serde_json::from_str::<crate::Index>(text).is_err());
}

#[test]
fn a_location_another_scenario_covers_leaves_the_chip_wide_untriaged_set() {
    let at = |function: &str, offset: u32| crate::Location {
        function: function.into(),
        offset,
        kind: crate::LocationKind::Block,
    };
    let shard = |untriaged: Vec<crate::Location>, functions: &[&str]| crate::Index {
        schema: crate::SCHEMA,
        command: crate::BLOBRAY.into(),
        target: "chip-a".into(),
        scenario: "test".into(),
        inputs: Default::default(),
        sources: vec![],
        dependence: crate::Dependence::whole_closure("test"),
        entries: vec![],
        untriaged,
        functions: functions.iter().map(|f| f.to_string()).collect(),
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    let shards = [
        // Leaves `shared+0` and `shared+4` and `own+0` untriaged.
        shard(
            vec![at("own", 0), at("shared", 0), at("shared", 4)],
            &["own", "shared"],
        ),
        // Covers `shared+0`, leaves `shared+4` untriaged too.
        shard(vec![at("shared", 4)], &["shared"]),
        // Contains neither function.
        shard(vec![at("other", 0)], &["other"]),
    ];
    assert_eq!(
        crate::untriaged(&shards.iter().collect::<Vec<_>>()),
        BTreeSet::from([at("other", 0), at("own", 0), at("shared", 4)])
    );
}

#[test]
fn only_a_decision_that_applies_to_a_shard_makes_it_stale() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::write(root.join("sources/lib.rs"), "one").unwrap();
    std::fs::create_dir_all(root.join("decisions")).unwrap();
    let decisions = PathBuf::from("decisions/coverage.toml");
    let file = |applied: &str, other: &str| {
        let text = format!(
            "[[decision]]\nreason = \"{applied}\"\n[[decision.place]]\nfunction = \"closure\"\n\
             start = 0x2\nend = 0x8\n\n[[decision]]\nreason = \"{other}\"\n\
             [[decision.place]]\nfunction = \"elsewhere\"\n"
        );
        std::fs::write(root.join(&decisions), text).unwrap();
    };
    file("applied", "other");
    let functions = vec!["closure".to_owned()];
    let digest = crate::CoverageDecisions::read(root, &decisions)
        .unwrap()
        .applicable_digest(&functions);
    let mut dependence = crate::Dependence::whole_closure("test");
    dependence.coverage_decisions = Some(crate::DecisionDigest {
        path: decisions.clone(),
        sha256: digest,
    });
    let shard = crate::Index {
        schema: crate::SCHEMA,
        command: crate::BLOBRAY.into(),
        target: "chip-a".into(),
        scenario: "leaf".into(),
        inputs: Default::default(),
        sources: vec![crate::SourceDigest {
            sha256: crate::digest_directory(root, Path::new("sources")).unwrap(),
            path: PathBuf::from("sources"),
        }],
        dependence,
        entries: vec![],
        untriaged: vec![],
        functions,
        unobserved: vec![],
        observed: vec![],
        unprojected: vec![],
    };
    shard.validate("chip-a").unwrap();
    file("applied", "edited");
    assert!(shard.is_current(root));
    file("edited", "edited");
    assert!(!shard.is_current(root));
    std::fs::write(
        root.join(&decisions),
        "[[decision]]\nreason = \"no place\"\nplace = []\n",
    )
    .unwrap();
    assert!(crate::CoverageDecisions::read(root, &decisions).is_err());
    assert!(!shard.is_current(root));
}
