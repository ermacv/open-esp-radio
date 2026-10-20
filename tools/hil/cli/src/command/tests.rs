use super::*;
use clap::Parser as _;

fn args(words: &[&str]) -> Vec<OsString> {
    words.iter().map(Into::into).collect()
}

#[test]
fn the_complete_tree_accepts_the_existing_host_and_runner_commands() {
    HilCli::command().debug_assert();
    for words in [
        vec![],
        vec!["--root=/c", "sweep", "--apply"],
        vec!["dashboard", "--port", "8765"],
        vec!["observer"],
        vec!["peer", "send", "peer-a", "CFG", "15", "--for", "5s"],
        vec!["firmware", "list"],
        vec!["firmware", "build", "peer"],
        vec!["images", "check", "--all", "--type-check"],
        vec!["runs", "list", "--scenario", "boot-smoke"],
        vec!["perf", "report", "--since", "3d"],
        vec!["profile", "run-1", "--top", "5"],
        vec!["wait", "job-1"],
        vec!["bisect", "HEAD", "--scenario", "boot-smoke"],
        vec![
            "ab",
            "--a",
            "rev=HEAD",
            "--b",
            "rev=HEAD~",
            "--scenario",
            "boot-smoke",
            "--enqueue",
            "--after",
            "job-1",
        ],
        vec![
            "run",
            "a",
            "b",
            "--repetitions",
            "2",
            "--owner=phy",
            "--enqueue",
            "--after-any=job-1",
        ],
        vec!["--owner", "phy", "run-all", "--all", "--after", "job-1"],
        vec!["plan", "boot-smoke", "--owner", "phy", "--root", "/c"],
        vec!["fixture", "check", "boot-smoke"],
        vec!["scenario", "list"],
    ] {
        assert!(HilCli::try_parse_from(&words).is_ok(), "{words:?}");
    }
    for words in [
        vec!["unknown"],
        vec!["run"],
        vec!["sweep", "--bad"],
        vec!["plan", "--enqueue"],
        vec!["run", "a", "--after=a", "--after-any=b"],
        vec!["run", "a", "--after=a", "--after=b"],
    ] {
        assert!(HilCli::try_parse_from(&words).is_err(), "{words:?}");
    }
}

#[test]
fn owners_in_all_positions_are_validated_and_must_agree() {
    for words in [
        vec!["--owner", "phy", "run", "a"],
        vec!["run", "a", "--owner=phy"],
        vec!["run", "--owner", "phy", "a"],
        vec!["--owner=phy", "run", "a", "--owner=phy"],
    ] {
        let cli = HilCli::try_parse_from(words).unwrap();
        assert_eq!(cli.owner.as_deref(), Some("phy"));
    }
    let cli = HilCli::try_parse_from(["--owner=old", "--owner=phy", "run", "a"]).unwrap();
    assert_eq!(cli.owner.as_deref(), Some("phy"));
    assert!(HilCli::try_parse_from(["--owner=phy", "run", "a", "--owner=bt"]).is_err());
    assert!(HilCli::try_parse_from(["run", "a", "--owner=Bad Name"]).is_err());
}

#[test]
fn forwarded_arguments_keep_values_and_exclude_host_options() {
    for words in [
        vec![
            "--board",
            "dut",
            "run",
            "a",
            "b",
            "--source=/tmp/src",
            "--features=+trace",
            "--repetitions=2",
            "--owner=phy",
            "--after-any=job",
        ],
        vec!["run-all", "--tag=wifi", "--exclude", "a", "--enqueue"],
        vec!["plan", "a", "--proof", "p", "--root=/c"],
        vec![
            "run",
            "a",
            "--then",
            "echo --owner=x; printf '%s' --enqueue",
        ],
        vec!["run", "--", "--owner"],
    ] {
        let matches = HilCli::command().try_get_matches_from(&words).unwrap();
        let cli = HilCli::from_arg_matches(&matches).unwrap();
        let HilCommand::Runner(original) = cli.command.unwrap() else {
            panic!("runner command")
        };
        let encoded = crate::arguments::encode(&oer_hil_run::cli::command(), &matches, &[]);
        let replay = oer_hil_run::cli::command()
            .no_binary_name(true)
            .try_get_matches_from(&encoded)
            .unwrap();
        let replay = oer_hil_run::cli::CliCommand::from_arg_matches(&replay).unwrap();
        assert_eq!(
            format!("{original:?}"),
            format!("{replay:?}"),
            "{words:?} -> {encoded:?}"
        );
        assert!(!encoded.contains(&OsString::from("--enqueue")));
    }
}

#[test]
fn fixed_images_replace_only_the_parsed_build_options() {
    let matches = HilCli::command()
        .try_get_matches_from([
            "run",
            "a",
            "--source",
            "/tmp/src",
            "--features=+trace",
            "--layout-seed",
            "3",
            "--then",
            "echo --source=keep",
            "--repetitions",
            "2",
        ])
        .unwrap();
    let encoded = crate::arguments::encode(
        &oer_hil_run::cli::command(),
        &matches,
        &["source", "layout_seed", "features"],
    );
    assert_eq!(
        encoded,
        args(&[
            "run",
            "--repetitions",
            "2",
            "--then",
            "echo --source=keep",
            "a"
        ])
    );
}

#[test]
fn profile_reports_stay_outside_finished_runs() {
    assert_eq!(
        profile_report_path(
            Path::new("/checkout"),
            "run-1",
            "udp",
            Path::new("repetition-001")
        ),
        Path::new("/checkout/target/hil/profiles/run-1/udp/repetition-001/profile.txt")
    );
}

#[test]
fn command_tree_keeps_the_json_contract_with_explicit_corrections() {
    use oer_os_command_tree::CommandNode;
    let mut expected: Vec<CommandNode> =
        serde_json::from_str(include_str!("../../tests/fixtures/command-tree.json")).unwrap();
    // The root now includes its checkout and the runner's existing globals.
    expected[0]
        .flags
        .extend(["--root", "--stand-file", "--board", "--peer-board"].map(String::from));
    // --owner is inherited from the root, rather than copied into run nodes.
    for node in &mut expected {
        if matches!(
            node.path.last().map(String::as_str),
            Some("run" | "run-all")
        ) {
            node.flags.retain(|flag| flag != "--owner");
        }
    }
    // Previously only the root's command list named these two commands.
    expected.extend([
        CommandNode {
            path: vec!["hil".into(), "observer".into()],
            subcommands: vec![],
            flags: vec![],
            forwards: false,
        },
        CommandNode {
            path: vec!["hil".into(), "sweep".into()],
            subcommands: vec![],
            flags: vec!["--apply".into()],
            forwards: false,
        },
    ]);
    let mut actual = oer_os_command_tree::command_tree(&HilCli::command(), &["hil".into()]);
    // Clap emits command nodes in depth-first declaration order.
    expected.sort_by(|a, b| a.path.cmp(&b.path));
    actual.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(actual, expected);
}

#[test]
fn parser_failures_keep_host_and_runner_exit_statuses() {
    for (words, status) in [
        (vec!["sweep", "--bad"], 1),
        (vec!["runs", "list", "--bad"], 1),
        (vec!["run"], 2),
        (vec!["run", "a", "--bad"], 2),
        (vec!["--owner"], 1),
        (vec!["--root"], 1),
        (vec!["unknown"], 2),
        (vec!["firmware", "--help"], 1),
        (vec!["--help"], 0),
    ] {
        let error = HilCli::try_parse_from(&words)
            .err()
            .expect("parser error or help");
        assert_eq!(usage_status(&error, &args(&words)), status, "{words:?}");
    }
}

#[cfg(unix)]
#[test]
fn forwarding_keeps_non_utf8_filesystem_paths() {
    use std::os::unix::ffi::OsStringExt as _;
    let path = OsString::from_vec(b"/tmp/source-\xff".to_vec());
    let words = ["run".into(), "a".into(), "--source".into(), path.clone()];
    let matches = HilCli::command().try_get_matches_from(words).unwrap();
    let encoded = crate::arguments::encode(&oer_hil_run::cli::command(), &matches, &[]);
    assert!(encoded.contains(&path));
    assert!(
        oer_hil_run::cli::command()
            .no_binary_name(true)
            .try_get_matches_from(encoded)
            .is_ok()
    );
}
