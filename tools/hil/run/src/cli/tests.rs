use super::*;

#[test]
fn service_requests_use_clap_and_stay_out_of_the_public_tree() {
    command().debug_assert();
    assert!(matches!(
        Invocation::try_parse_from(["runner", "--observer-build"])
            .expect("accepted runner invocation"),
        Invocation::ObserverBuild
    ));
    assert!(matches!(
        Invocation::try_parse_from(["runner", "__command-tree"])
            .expect("accepted runner invocation"),
        Invocation::CommandTree
    ));
    let Invocation::Run(cli) =
        Invocation::try_parse_from(["runner", "run", "boot-smoke", "--board", "s31-a"])
            .expect("accepted runner invocation")
    else {
        panic!("expected a scenario run");
    };
    assert_eq!(cli.board.as_deref(), Some("s31-a"));
    for arguments in [
        vec!["runner", "--observer-build", "--bad"],
        vec!["runner", "__command-tree", "--bad"],
        vec!["runner", "unknown", "--observer-build"],
        vec!["runner", "run", "boot-smoke", "--observer-build"],
    ] {
        assert!(
            Invocation::try_parse_from(&arguments).is_err(),
            "{arguments:?}"
        );
    }
    use clap::CommandFactory as _;
    assert_eq!(
        oer_os_command_tree::command_tree(&command(), &[]),
        oer_os_command_tree::command_tree(&Cli::command(), &[])
    );
}

#[test]
fn a_source_tree_is_a_run_flag_that_replaying_firmware_excludes() {
    let run =
        Cli::try_parse_from(["cargo-hil", "run", "boot-smoke", "--source", "/tmp/arm-a"]).unwrap();
    assert!(matches!(
        run.command,
        CliCommand::Run {
            source: Some(ref source),
            ..
        } if source == std::path::Path::new("/tmp/arm-a")
    ));
    assert!(
        Cli::try_parse_from([
            "cargo-hil",
            "run",
            "boot-smoke",
            "--source",
            "/tmp/arm-a",
            "--firmware-from",
            "run-1",
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["cargo-hil", "image", "snapshot"]).is_err());
}

#[test]
fn preflight_selection_is_unambiguous() {
    for command in ["plan", "doctor"] {
        assert!(Cli::try_parse_from(["cargo-hil", command, "timebase"]).is_ok());
        assert!(Cli::try_parse_from(["cargo-hil", command, "--tag", "system"]).is_ok());
        assert!(
            Cli::try_parse_from(["cargo-hil", command, "timebase", "--tag", "system"]).is_err()
        );
    }
}

#[test]
fn run_firmware_from_is_an_explicit_single_scenario_input() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "run",
        "station-udp-rx-ceiling",
        "--firmware-from",
        "sealed-run-1",
    ])
    .unwrap();
    match cli.command {
        CliCommand::Run {
            scenarios,
            firmware_from,
            ..
        } => {
            assert_eq!(scenarios, ["station-udp-rx-ceiling"]);
            assert_eq!(firmware_from.as_deref(), Some("sealed-run-1"));
        }
        _ => panic!("parsed the wrong HIL command"),
    }
}

#[test]
fn run_takes_scenarios_in_order_and_requires_one() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "b", "a"]).unwrap();
    match cli.command {
        CliCommand::Run { scenarios, .. } => assert_eq!(scenarios, ["b", "a"]),
        _ => panic!("parsed the wrong HIL command"),
    }
    assert!(Cli::try_parse_from(["cargo-hil", "run"]).is_err());
}

#[test]
fn removed_commands_and_flags_are_rejected() {
    for removed in [
        &["cargo-hil", "run", "a", "--network", "owned-xarxa"][..],
        &["cargo-hil", "run", "a", "--ap-scheduler", "rr"],
        &["cargo-hil", "run-plan", "plan.json"],
        &["cargo-hil", "image", "flash", "system-watchdog"],
        &["cargo-hil", "image", "replay", "run", "system-watchdog"],
        &["cargo-hil", "image", "mono", "system-watchdog"],
        &["cargo-hil", "image", "verify-rebuild", "system-watchdog"],
        &["cargo-hil", "device", "status"],
        &["cargo-hil", "report", "rebuild"],
        &["cargo-hil", "report", "verify", "run-1"],
        &["cargo-hil", "archive", "verify", "a.tar.gz"],
        &["cargo-hil", "fixture", "probe-plan"],
        &["cargo-hil", "plan", "--out", "plan.json"],
    ] {
        assert!(Cli::try_parse_from(removed).is_err(), "{removed:?}");
    }
}

#[test]
fn run_all_does_not_accept_one_ambiguous_firmware_origin() {
    assert!(
        Cli::try_parse_from(["cargo-hil", "run-all", "--firmware-from", "sealed-run-1",]).is_err()
    );
}

#[test]
fn fixture_commands_parse_a_scenario_check() {
    assert!(Cli::try_parse_from(["cargo-hil", "fixture", "install-host"]).is_err());
    assert!(Cli::try_parse_from(["cargo-hil", "fixture", "bluetooth-check"]).is_err());
    let cli =
        Cli::try_parse_from(["cargo-hil", "fixture", "check", "station-udp-tx-he20"]).unwrap();
    assert!(matches!(
        cli.command,
        CliCommand::Fixture { command: FixtureCommand::Check { scenario, chip: None } }
            if scenario == "station-udp-tx-he20"
    ));
}

#[test]
fn run_all_runs_the_whole_catalog_only_when_asked() {
    assert!(Cli::try_parse_from(["cargo-hil", "run-all"]).is_err());
    assert!(Cli::try_parse_from(["cargo-hil", "run-all", "--tag", "wifi"]).is_ok());
    assert!(Cli::try_parse_from(["cargo-hil", "run-all", "--all"]).is_ok());
}

#[test]
fn run_repetitions_replace_the_scenarios_counts_within_the_evidence_range() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "a", "--repetitions", "2"]).unwrap();
    let CliCommand::Run { repetitions, .. } = cli.command else {
        panic!("expected run");
    };
    assert_eq!(repetitions, Some(2));
    for out_of_range in ["0", "21"] {
        assert!(
            Cli::try_parse_from(["cargo-hil", "run", "a", "--repetitions", out_of_range]).is_err()
        );
    }
}

#[test]
fn a_build_only_run_builds_into_a_directory_and_is_no_replay() {
    let cli =
        Cli::try_parse_from(["cargo-hil", "run", "a", "--build-only", "/jobs/7/images"]).unwrap();
    let CliCommand::Run { build_only, .. } = cli.command else {
        panic!("expected run");
    };
    assert_eq!(build_only, Some(std::path::PathBuf::from("/jobs/7/images")));
    for conflicting in [["--firmware-from", "r"], ["--images", "/jobs/6/images"]] {
        assert!(
            Cli::try_parse_from(
                ["cargo-hil", "run", "a", "--build-only", "/jobs/7/images"]
                    .into_iter()
                    .chain(conflicting)
            )
            .is_err()
        );
    }
}

#[test]
fn a_run_of_built_images_takes_neither_a_tree_nor_build_options() {
    let cli = Cli::try_parse_from(["cargo-hil", "run", "a", "--images", "/jobs/7/images"]).unwrap();
    let CliCommand::Run { images, .. } = cli.command else {
        panic!("expected run");
    };
    assert_eq!(images, Some(std::path::PathBuf::from("/jobs/7/images")));
    for conflicting in [
        ["--source", "/tmp/arm-a"],
        ["--layout-seed", "3"],
        ["--features", "+trace"],
        ["--firmware-from", "r"],
    ] {
        assert!(
            Cli::try_parse_from(
                ["cargo-hil", "run", "a", "--images", "/jobs/7/images"]
                    .into_iter()
                    .chain(conflicting)
            )
            .is_err()
        );
    }
}

#[test]
fn run_all_takes_repeated_exclusions() {
    let cli = Cli::try_parse_from([
        "cargo-hil",
        "run-all",
        "--all",
        "--exclude",
        "a",
        "--exclude",
        "b",
    ])
    .unwrap();
    let CliCommand::RunAll { exclude, .. } = cli.command else {
        panic!("expected run-all");
    };
    assert_eq!(exclude, ["a", "b"]);
}

#[test]
fn run_all_selects_by_role() {
    let cli = Cli::try_parse_from(["cargo-hil", "run-all", "--role", "qualification"]).unwrap();
    let CliCommand::RunAll { role, .. } = cli.command else {
        panic!("expected run-all");
    };
    assert_eq!(role.as_deref(), Some("qualification"));
    assert!(Cli::try_parse_from(["cargo-hil", "run-all", "--role", "triage"]).is_err());
}

#[test]
fn the_image_command_offers_only_what_it_has() {
    use clap::CommandFactory as _;
    let command = Cli::command();
    let image = command.find_subcommand("image").unwrap();
    let names: Vec<_> = image.get_subcommands().map(|sub| sub.get_name()).collect();
    assert!(!names.contains(&"flash"));
    let about = image.get_about().unwrap().to_string();
    assert!(!about.contains("flash"), "{about}");
}
