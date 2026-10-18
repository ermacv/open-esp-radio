use super::*;

#[test]
fn identifiers_skip_short_words_and_numbers() {
    let mut words = BTreeSet::new();
    identifiers(
        "// SOURCE: complete `libpp.a[trc.o]::rcGetRate`, size 0xd0; in 2 of",
        &mut words,
    );
    assert!(words.contains("rcGetRate"));
    assert!(words.contains("SOURCE"));
    identifiers("the ROM main loop may abort ru2str", &mut words);
    assert!(!words.contains("main") && !words.contains("abort"));
    assert!(words.contains("ru2str"));
    assert!(!words.contains("of"));
    assert!(!words.contains("0xd0"));
    identifiers("phy_init.o (sha256 …) phy_rf_init", &mut words);
    assert!(
        !words.contains("phy_init"),
        "an archive member name is not a function"
    );
    assert!(words.contains("phy_rf_init"));
}

#[test]
fn only_comment_blocks_opened_by_the_marker_are_cited() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// Unrelated rcGetRate prose.\n\
         fn x() {}\n\
         /// SOURCE: complete\n\
         /// `hal_mac_tx_set_ppdu` body.\n\
         fn y() {}\n\
         // rcUpdateRate after the block\n",
    )
    .unwrap();
    let chip_dir = directory.path().join("chip-a");
    std::fs::create_dir(&chip_dir).unwrap();
    std::fs::rename(directory.path().join("a.rs"), chip_dir.join("a.rs")).unwrap();
    let words = scan(directory.path(), "chip-a").0;
    assert!(words.contains("hal_mac_tx_set_ppdu"));
    assert!(!words.contains("rcGetRate"));
    assert!(!words.contains("rcUpdateRate"));
}

fn supported() -> Vec<String> {
    vec!["chip-b".into(), "chip-a".into()]
}

/// Another chip's facts cite its own pins, so they are not this chip's
/// references.
#[test]
fn another_chips_directory_is_not_scanned() {
    let directory = tempfile::tempdir().unwrap();
    for (chip, symbol) in [
        ("chip-a", "bt_bb_v2_init_cmplx"),
        ("chip-b", "ieee802154_txon_delay_set"),
    ] {
        let crate_dir = directory.path().join("hardware").join(chip);
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(
            crate_dir.join("a.rs"),
            format!("// SOURCE: `{symbol}` body.\nfn x() {{}}\n"),
        )
        .unwrap();
    }
    let (words, problems) = scan(directory.path(), "chip-a");
    assert!(words.contains("bt_bb_v2_init_cmplx"));
    assert!(!words.contains("ieee802154_txon_delay_set"));
    assert!(problems.is_empty(), "{problems:?}");
}

/// Words and violations of a scan of `chip` under `directory`.
fn scan(directory: &Path, chip: &str) -> (BTreeSet<String>, Vec<String>) {
    let supported = supported();
    let mut found = Found::default();
    production_words(
        &Scan {
            chip,
            supported: &supported,
            citable: &supported,
        },
        &oer_repo::Repo::from_dir(directory).unwrap(),
        "",
        &mut found,
    )
    .unwrap();
    let mut problems = found.problems;
    for (location, _) in found.uncharted {
        problems.push(format!("{location}: names no chip"));
    }
    (found.words, problems)
}

/// A chip-neutral block cites only the chips it names, and names at least one.
#[test]
fn neutral_blocks_cite_the_chips_they_name() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// SOURCE(chip-a): `bt_bb_v2_init_cmplx`\nfn x() {}\n\
         // SOURCE(chip-a,chip-b)[TAG]: `phy_get_data_sat`\nfn y() {}\n\
         // SOURCE: `phy_i2c_init1`\nfn z() {}\n",
    )
    .unwrap();
    let (dut, s31_problems) = scan(directory.path(), "chip-a");
    let (peer, c5_problems) = scan(directory.path(), "chip-b");
    assert!(dut.contains("bt_bb_v2_init_cmplx") && dut.contains("phy_get_data_sat"));
    assert!(!peer.contains("bt_bb_v2_init_cmplx") && peer.contains("phy_get_data_sat"));
    assert!(!dut.contains("phy_i2c_init1") && !peer.contains("phy_i2c_init1"));
    assert!(
        !dut.contains("chip-b"),
        "chip names are not function candidates"
    );
    for problems in [s31_problems, c5_problems] {
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("a.rs:5") && problems[0].contains("names no chip"));
    }
}

#[test]
fn the_registry_round_trips() {
    let entries = vec![Entry {
        artifact: "libbtbb".into(),
        member: "bt_bb_v2.o".into(),
        symbol: "bt_bb_v2_init_cmplx".into(),
        code: "ab".into(),
        decisions: vec![],
    }];
    assert_eq!(parse_registry(&render_registry(&entries)).unwrap(), entries);
}

/// An untagged neutral block is a violation of exactly the chips whose pins
/// define a function it names; a block citing only a standard passes.
#[test]
fn an_untagged_neutral_block_fails_only_the_chips_it_cites() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("a.rs"),
        "// SOURCE: `phy_i2c_init1` body\nfn x() {}\n\
         // SOURCE: IEEE Std 802.11-2020 12.7.1.6.2 (KDF)\nfn y() {}\n",
    )
    .unwrap();
    let supported = supported();
    let mut found = Found::default();
    production_words(
        &Scan {
            chip: "chip-b",
            supported: &supported,
            citable: &supported,
        },
        &oer_repo::Repo::from_dir(directory.path()).unwrap(),
        "",
        &mut found,
    )
    .unwrap();
    assert_eq!(found.uncharted.len(), 2);
    let peer: BTreeSet<&str> = ["phy_i2c_init1"].into();
    let dut: BTreeSet<&str> = ["bt_bb_v2_init_cmplx"].into();
    let c5_problems = uncharted_citations(&found.uncharted, &peer);
    assert_eq!(c5_problems.len(), 1, "{c5_problems:?}");
    assert!(c5_problems[0].starts_with("a.rs:1") && c5_problems[0].contains("phy_i2c_init1"));
    assert!(uncharted_citations(&found.uncharted, &dut).is_empty());
}

#[test]
fn decisions_cite_the_code_shaped_names_their_places_quote() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("coverage.toml"),
        "[[decision]]\nreason = \"\"\"\nthe TXOP holding path, \\\nnot used\"\"\"\n\
         [[decision.place]]\nfunction = \"mac_tx_set_txop_q\"\nstart = 0x30\nend = 0x34\n\
         [[decision.place]]\nfunction = \"hal_mac_fill_hwtxop\"\n",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("state.rs"),
        r#"("phy/src/target_port.rs", "self.registers,"), Place::Function("mac_tx_set_txop_q")"#,
    )
    .unwrap();
    std::fs::write(directory.path().join("notes.md"), r#""ignored_file_name""#).unwrap();
    let words = decision_words(directory.path()).unwrap();
    assert_eq!(
        words.keys().collect::<Vec<_>>(),
        ["hal_mac_fill_hwtxop", "mac_tx_set_txop_q"]
    );
    assert_eq!(
        words["mac_tx_set_txop_q"].iter().collect::<Vec<_>>(),
        ["coverage.toml", "state.rs"]
    );
}

#[test]
fn a_changed_decision_function_names_the_exclusions_to_review() {
    let files: BTreeSet<String> = ["coverage.toml".to_owned()].into();
    assert_eq!(
        review_hint(Some(&files)),
        "; re-review the exclusions in decisions/coverage.toml"
    );
    assert_eq!(review_hint(None), "");
}

#[test]
fn the_registry_round_trips_decision_citations() {
    let entries = vec![Entry {
        artifact: "libpp".into(),
        member: "hal_mac_tx.o".into(),
        symbol: "mac_tx_set_txop_q".into(),
        code: "abc".into(),
        decisions: vec!["coverage.rs".into()],
    }];
    assert_eq!(parse_registry(&render_registry(&entries)).unwrap(), entries);
}

#[test]
fn accepted_functions_take_the_printed_form() {
    assert_eq!(
        accepted_function("phy_write_pll_cap"),
        (None, "phy_write_pll_cap")
    );
    assert_eq!(
        accepted_function("libnet80211[wl_cnx.o]::sta_reset_beacon_timeout"),
        (
            Some(("libnet80211", "wl_cnx.o")),
            "sta_reset_beacon_timeout"
        )
    );
    assert_eq!(
        accepted_function("rom[]::sta_reset_beacon_timeout"),
        (Some(("rom", "")), "sta_reset_beacon_timeout")
    );
}

#[test]
fn accepting_reports_how_the_fingerprint_moves() {
    let entry = |code: &str| Entry {
        artifact: "libpp".into(),
        member: "pp.o".into(),
        symbol: "ppTxPkt".into(),
        code: code.into(),
        decisions: vec![],
    };
    assert_eq!(
        fingerprint_move(&entry("b"), &[]),
        "libpp[pp.o]::ppTxPkt: newly registered at b"
    );
    assert_eq!(
        fingerprint_move(&entry("b"), &[entry("b")]),
        "libpp[pp.o]::ppTxPkt: unchanged at b"
    );
    assert_eq!(
        fingerprint_move(&entry("b"), &[entry("a")]),
        "libpp[pp.o]::ppTxPkt: a -> b"
    );
}

#[test]
fn rom_summaries_cite_their_functions_whatever_the_name_s_shape() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("functions.toml");
    assert!(summary_names(&path).unwrap().is_empty());
    std::fs::write(
        &path,
        "[[function]]\nname = \"memset\"\naddress = 1\nframe = 0\n",
    )
    .unwrap();
    // A plain lowercase word is prose in a SOURCE block, a citation here.
    assert_eq!(
        summary_names(&path).unwrap(),
        BTreeSet::from(["memset".to_owned()])
    );
    std::fs::write(&path, "[[function]]\naddress = 1\n").unwrap();
    assert!(summary_names(&path).is_err());
}

#[test]
fn a_qualified_citation_names_one_copy_and_leaves_the_rest_of_the_text_bare() {
    let (bare, copies) = qualified_citations(
        "`libpp[pm.o]::pm_scale_listen_interval` and `rom[]::pm_tbtt_process`; \
         `pm_parse_beacon` bare, `x[y]` no citation, `libpp.a[pp.o]::sta_input` a file",
    );
    assert_eq!(
        copies,
        [
            (
                "libpp".into(),
                "pm.o".into(),
                "pm_scale_listen_interval".into()
            ),
            ("rom".into(), String::new(), "pm_tbtt_process".into()),
            ("libpp.a".into(), "pp.o".into(), "sta_input".into()),
        ]
    );
    let mut words = BTreeSet::new();
    identifiers(&bare, &mut words);
    assert!(words.contains("pm_parse_beacon"));
    assert!(!words.contains("pm_scale_listen_interval"));
    assert!(!words.contains("pm_tbtt_process"));
}

/// A survey of one function with a library and a ROM copy, both registered.
fn two_copies(references: &[&str], qualified: &[(&str, &str)]) -> Survey {
    let code = |copy: &str| format!("code of {copy}");
    let entry = |artifact: &str, member: &str| Entry {
        artifact: artifact.into(),
        member: member.into(),
        symbol: "pm_parse_beacon".into(),
        code: code(artifact),
        decisions: vec![],
    };
    let mut copies: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut qualified_at = vec![];
    for (artifact, member) in qualified {
        copies
            .entry("pm_parse_beacon".into())
            .or_default()
            .insert(((*artifact).into(), (*member).into()));
        qualified_at.push((
            "a.rs:1".into(),
            (*artifact).into(),
            (*member).into(),
            "pm_parse_beacon".into(),
        ));
    }
    Survey {
        citations: vec![],
        registry: vec![entry("libpp", "pm.o"), entry("rom", "")],
        current: BTreeMap::from([
            (
                ("libpp".into(), "pm.o".into(), "pm_parse_beacon".into()),
                code("libpp"),
            ),
            (
                ("rom".into(), String::new(), "pm_parse_beacon".into()),
                code("rom"),
            ),
        ]),
        references: references.iter().map(|r| (*r).into()).collect(),
        qualified: copies,
        qualified_at,
        bare_at: vec![(
            "a.rs:1".into(),
            references.iter().map(|r| (*r).into()).collect(),
        )],
        words: BTreeSet::new(),
        decisions: BTreeMap::new(),
        pinned: BTreeMap::new(),
        documents: vec![],
    }
}

#[test]
fn a_bare_name_of_a_function_with_two_copies_is_an_error() {
    // Both copies stay cited and registered, but the citation names neither.
    let survey = two_copies(&["pm_parse_beacon"], &[]);
    assert_eq!(
        problems_of(&survey),
        [
            "a.rs:1: cites `pm_parse_beacon` bare, but 2 pinned copies define it \
          (libpp[pm.o]::pm_parse_beacon, rom[]::pm_parse_beacon); cite the copy the fact \
          was read from"
        ]
    );
}

#[test]
fn a_bare_name_of_a_function_with_one_copy_cites_it() {
    let mut survey = two_copies(&["pm_parse_beacon"], &[]);
    survey
        .current
        .retain(|(artifact, _, _), _| artifact == "libpp");
    survey.registry.retain(|e| e.artifact == "libpp");
    assert!(problems_of(&survey).is_empty());
}

#[test]
fn a_qualified_citation_requires_only_its_copy() {
    let mut survey = two_copies(&[], &[("libpp", "pm.o")]);
    let problems = problems_of(&survey);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].starts_with("rom[]::pm_parse_beacon is registered but no longer cited"));
    survey.registry.retain(|e| e.artifact == "libpp");
    assert!(problems_of(&survey).is_empty());
}

#[test]
fn a_qualified_citation_of_no_pinned_copy_is_an_error() {
    let mut survey = two_copies(&[], &[("libpp", "pm_typo.o")]);
    survey.registry.clear();
    let problems = problems_of(&survey);
    assert_eq!(
        problems,
        ["a.rs:1: cites libpp[pm_typo.o]::pm_parse_beacon, which no pinned artifact defines"]
    );
}

#[test]
fn a_form_of_an_unpinned_artifact_cites_its_symbol_bare() {
    let mut bare_at = vec![];
    let qualified = split_forms(
        vec![
            (
                "a.rs:1".into(),
                "libpp".into(),
                "pm.o".into(),
                "pm_parse_beacon".into(),
            ),
            (
                "a.rs:2".into(),
                "libpp.a".into(),
                "pm.o".into(),
                "pm_tbtt_process".into(),
            ),
        ],
        |artifact| artifact == "libpp",
        &mut bare_at,
    );
    assert_eq!(
        qualified,
        [(
            "a.rs:1".into(),
            "libpp".into(),
            "pm.o".into(),
            "pm_parse_beacon".into()
        )]
    );
    assert_eq!(
        bare_at,
        [(
            "a.rs:2".to_owned(),
            BTreeSet::from(["pm_tbtt_process".to_owned()])
        )]
    );
}

#[test]
fn a_rom_summary_cites_the_rom_copy_only() {
    let mut qualified = BTreeMap::new();
    rom_summary_citations(
        BTreeSet::from(["pm_parse_beacon".to_owned()]),
        "rom",
        &mut qualified,
    );
    let mut survey = two_copies(&[], &[]);
    survey.qualified = qualified;
    survey.bare_at.clear();
    // The library copy the summary does not describe is no longer cited.
    let problems = problems_of(&survey);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("libpp[pm.o]::pm_parse_beacon is registered but no longer cited")
    );
}
