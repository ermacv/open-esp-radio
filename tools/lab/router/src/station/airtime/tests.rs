use super::*;

/// A router's mac80211 debugfs as these scripts use it: `aql_txq_limit`
/// takes `<ac> <low> <high>` and prints every category; `aql_threshold`
/// prints one number; a station's `aql` prints its pending airtime and
/// limits. `ignore_writes` models a router that does not apply a write.
struct Router {
    limits: [Limit; 4],
    threshold_us: u32,
    station: Option<([u32; 4], [Limit; 4])>,
    ignore_writes: bool,
    has_aql: bool,
    /// The connection breaks after the writes ran, before the read-back.
    break_after_writes: bool,
    scripts: usize,
}

impl Router {
    fn at_defaults() -> Self {
        Self {
            limits: [DEFAULT; 4],
            threshold_us: 24000,
            station: None,
            ignore_writes: false,
            has_aql: true,
            break_after_writes: false,
            scripts: 0,
        }
    }

    fn print_limits(&self) -> String {
        let mut text = String::from("AC\tAQL limit low\tAQL limit high\n");
        for (ac, limit) in AccessCategory::ALL.iter().zip(self.limits) {
            text.push_str(&format!(
                "{}\t{}\t\t{}\n",
                ac.label(),
                limit.low_us,
                limit.high_us
            ));
        }
        text.push_str("BC/MC\t50000\n");
        text
    }
}

impl Remote for Router {
    fn run(&mut self, script: &str) -> Result<String> {
        self.scripts += 1;
        if script.contains("then echo present; else echo absent; fi") {
            return Ok(if self.has_aql {
                "present\n"
            } else {
                "absent\n"
            }
            .to_owned());
        }
        let mut out = String::new();
        let mut wrote = false;
        for line in script.lines() {
            if let Some(write) = line
                .strip_prefix("echo 'limit ")
                .and_then(|rest| rest.split_once("' > "))
                .filter(|(_, file)| file.ends_with("/aql\""))
                .map(|(write, _)| write)
            {
                let values: Vec<u32> = write.split(' ').map(|v| v.parse().unwrap()).collect();
                let (_, limits) = self.station.as_mut().ok_or("no such station file")?;
                limits[values[0] as usize] = Limit {
                    low_us: values[1],
                    high_us: values[2],
                };
                wrote = true;
                continue;
            }
            if wrote && self.break_after_writes && line.starts_with("echo '[") {
                return Err("ssh: connection closed".to_owned());
            }
            if let Some(write) = line
                .strip_prefix("echo '")
                .and_then(|rest| rest.strip_suffix("' > \"$d/aql_txq_limit\""))
            {
                let values: Vec<u32> = write.split(' ').map(|v| v.parse().unwrap()).collect();
                if !self.ignore_writes {
                    self.limits[values[0] as usize] = Limit {
                        low_us: values[1],
                        high_us: values[2],
                    };
                }
                wrote = true;
            } else if let Some(name) = line
                .strip_prefix("echo '")
                .and_then(|r| r.strip_suffix('\''))
            {
                out.push_str(name);
                out.push('\n');
            } else if line == "cat \"$d/aql_txq_limit\"" {
                out.push_str(&self.print_limits());
            } else if line == "cat \"$d/aql_threshold\"" {
                out.push_str(&format!("{}\n", self.threshold_us));
            } else if line.starts_with("cat \"$d/netdev:") {
                let (pending, limits) = self.station.ok_or("no such station file")?;
                out.push_str(&format!(
                    "Q depth: VO: {} us VI: {} us BE: {} us BK: {} us\n\
                     Q limit[low/high]: VO: {}/{} VI: {}/{} BE: {}/{} BK: {}/{}\n",
                    pending[0],
                    pending[1],
                    pending[2],
                    pending[3],
                    limits[0].low_us,
                    limits[0].high_us,
                    limits[1].low_us,
                    limits[1].high_us,
                    limits[2].low_us,
                    limits[2].high_us,
                    limits[3].low_us,
                    limits[3].high_us,
                ));
            }
        }
        Ok(out)
    }
}

const RAISED: Limit = Limit {
    low_us: 5000,
    high_us: 18000,
};

#[test]
fn a_set_changes_only_its_category_and_reads_back_every_limit() {
    let mut router = Router::at_defaults();
    let read = set(&mut router, "radio0", AccessCategory::Be, RAISED, None).unwrap();
    assert_eq!(read.radio.be, RAISED);
    assert_eq!(read.radio.vo, DEFAULT);
    assert_eq!(read.radio.vi, DEFAULT);
    assert_eq!(read.radio.bk, DEFAULT);
    assert_eq!(read.threshold_us, 24000);
    assert_eq!(read.station, None);
    assert_eq!(router.limits[2], RAISED);
}

#[test]
fn a_write_the_router_did_not_apply_is_an_error_not_a_success() {
    let mut router = Router {
        ignore_writes: true,
        ..Router::at_defaults()
    };
    assert!(set(&mut router, "radio0", AccessCategory::Be, RAISED, None).is_err());
    assert!(
        restore_defaults(
            &mut Router {
                limits: [RAISED; 4],
                ignore_writes: true,
                ..Router::at_defaults()
            },
            "radio0"
        )
        .is_err()
    );
}

#[test]
fn a_limit_outside_the_lab_bounds_never_reaches_the_router() {
    for limit in [
        Limit {
            low_us: 0,
            high_us: 12000,
        },
        Limit {
            low_us: 13000,
            high_us: 12000,
        },
        Limit {
            low_us: 5000,
            high_us: MAX_HIGH_US + 1,
        },
    ] {
        let mut router = Router::at_defaults();
        assert!(set(&mut router, "radio0", AccessCategory::Be, limit, None).is_err());
        assert_eq!(router.scripts, 0, "{limit:?}");
        assert_eq!(router.limits, [DEFAULT; 4]);
    }
}

#[test]
fn restoring_returns_every_category_to_the_defaults_and_confirms_it() {
    let mut router = Router {
        limits: [
            RAISED,
            Limit {
                low_us: 1000,
                high_us: 2000,
            },
            RAISED,
            RAISED,
        ],
        ..Router::at_defaults()
    };
    let read = restore_defaults(&mut router, "radio0").unwrap();
    assert_eq!(router.limits, [DEFAULT; 4]);
    for ac in AccessCategory::ALL {
        assert_eq!(read.radio.get(ac), DEFAULT);
    }
}

#[test]
fn a_station_read_names_its_own_limits_and_pending_airtime() {
    let mut router = Router {
        limits: [DEFAULT, DEFAULT, RAISED, DEFAULT],
        station: Some(([0, 0, 12052, 0], [DEFAULT, DEFAULT, DEFAULT, DEFAULT])),
        ..Router::at_defaults()
    };
    let mac = Mac::parse("02:00:00:00:00:01").unwrap();
    let read = read(&mut router, "radio0", Some(("phy0-ap0", mac))).unwrap();
    // The radio's category and the station's own may differ: a station with
    // customized limits keeps them when the radio's change.
    assert_eq!(read.radio.be, RAISED);
    let station = read.station.unwrap();
    assert_eq!(station.limits.be, DEFAULT);
    assert_eq!(station.pending_us.be, 12052);
}

#[test]
fn an_unreadable_answer_is_an_error() {
    for text in [
        "",
        "[limits]\nVO\t5000\t\t12000\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\nBE 1 2\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE x 2\nBK 1 2\n[threshold]\n24000\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n",
        "[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n24000\n[station]\nQ depth: VO: 0 us\n",
        "stray\n[limits]\nVO 1 2\nVI 1 2\nBE 1 2\nBK 1 2\n[threshold]\n24000\n",
    ] {
        assert!(parse(text).is_err(), "{text:?}");
    }
}

#[test]
fn a_radio_says_whether_it_has_airtime_limits() {
    assert!(present(&mut Router::at_defaults(), "radio0").unwrap());
    assert!(
        !present(
            &mut Router {
                has_aql: false,
                ..Router::at_defaults()
            },
            "radio0"
        )
        .unwrap()
    );
}

/// One record the journal kept.
struct Kept {
    phase: Phase,
    changes: Vec<Change>,
    station: bool,
    restored: bool,
    reason: Option<String>,
}

/// A journal that keeps every record and can refuse one phase.
#[derive(Default)]
struct Journal {
    kept: Vec<Kept>,
    refuse: Option<Phase>,
}

impl Journal {
    fn recorder(&mut self) -> impl FnMut(&Record<'_>) -> Result<()> + '_ {
        move |record| {
            if self.refuse == Some(record.phase) {
                return Err("journal full".to_owned());
            }
            self.kept.push(Kept {
                phase: record.phase,
                changes: record.changes.to_vec(),
                station: record.station,
                restored: record.restored,
                reason: record.reason.map(str::to_owned),
            });
            Ok(())
        }
    }

    fn phases(&self) -> Vec<Phase> {
        self.kept.iter().map(|kept| kept.phase).collect()
    }
}

fn station() -> Option<(&'static str, Mac)> {
    Some(("phy0-ap0", Mac::parse("02:00:00:00:00:01").unwrap()))
}

#[test]
fn a_journaled_set_records_its_intent_before_the_write_and_its_confirmation_after() {
    let mut router = Router::at_defaults();
    let mut journal = Journal::default();
    set_recorded(
        &mut router,
        "radio0",
        AccessCategory::Be,
        RAISED,
        None,
        &mut journal.recorder(),
    )
    .unwrap();
    assert_eq!(journal.phases(), [Phase::Intended, Phase::Confirmed]);
    let change = Change {
        ac: AccessCategory::Be,
        before: DEFAULT,
        after: RAISED,
    };
    assert_eq!(journal.kept[0].changes, [change]);
    assert_eq!(journal.kept[1].changes, [change]);
    assert_eq!(router.limits[2], RAISED);
}

#[test]
fn an_intent_the_journal_refuses_writes_nothing() {
    let mut router = Router::at_defaults();
    let mut journal = Journal {
        refuse: Some(Phase::Intended),
        ..Journal::default()
    };
    assert!(
        set_recorded(
            &mut router,
            "radio0",
            AccessCategory::Be,
            RAISED,
            None,
            &mut journal.recorder()
        )
        .is_err()
    );
    assert_eq!(router.limits, [DEFAULT; 4]);
    assert!(journal.kept.is_empty());
    let mut changed = Router {
        limits: [RAISED; 4],
        ..Router::at_defaults()
    };
    assert!(restore_recorded(&mut changed, "radio0", &mut journal.recorder()).is_err());
    assert_eq!(changed.limits, [RAISED; 4]);
}

#[test]
fn a_write_whose_read_back_breaks_is_journaled_unconfirmed_and_returns_its_error() {
    let mut router = Router {
        break_after_writes: true,
        ..Router::at_defaults()
    };
    let mut journal = Journal::default();
    let error = set_recorded(
        &mut router,
        "radio0",
        AccessCategory::Be,
        RAISED,
        None,
        &mut journal.recorder(),
    )
    .unwrap_err();
    assert!(error.contains("unconfirmed"), "{error}");
    // The router did change: the journal holds the intent and an
    // unconfirmed result naming why.
    assert_eq!(router.limits[2], RAISED);
    assert_eq!(journal.phases(), [Phase::Intended, Phase::Unconfirmed]);
    assert!(
        journal.kept[1]
            .reason
            .as_deref()
            .unwrap()
            .contains("connection closed")
    );
    // The same holds when the unconfirmed record itself is refused: the
    // error says so.
    let mut router = Router {
        break_after_writes: true,
        ..Router::at_defaults()
    };
    let mut journal = Journal {
        refuse: Some(Phase::Unconfirmed),
        ..Journal::default()
    };
    let error = set_recorded(
        &mut router,
        "radio0",
        AccessCategory::Be,
        RAISED,
        None,
        &mut journal.recorder(),
    )
    .unwrap_err();
    assert!(error.contains("did not take"), "{error}");
}

#[test]
fn a_restore_is_journaled_only_for_the_categories_that_change() {
    let mut router = Router {
        limits: [DEFAULT, DEFAULT, RAISED, DEFAULT],
        ..Router::at_defaults()
    };
    let mut journal = Journal::default();
    restore_recorded(&mut router, "radio0", &mut journal.recorder()).unwrap();
    assert_eq!(router.limits, [DEFAULT; 4]);
    assert_eq!(journal.phases(), [Phase::Intended, Phase::Confirmed]);
    assert_eq!(
        journal.kept[0].changes,
        [Change {
            ac: AccessCategory::Be,
            before: RAISED,
            after: DEFAULT
        }]
    );
    assert!(journal.kept[0].restored, "a restore");
    // At the defaults already: nothing written, nothing journaled.
    let mut journal = Journal::default();
    let scripts = router.scripts;
    restore_recorded(&mut router, "radio0", &mut journal.recorder()).unwrap();
    assert!(journal.kept.is_empty());
    assert_eq!(router.scripts, scripts + 1, "one read, no write");
}

#[test]
fn a_station_with_its_own_limits_gets_the_written_ones_too() {
    // mac80211 keeps a station's limits when they differ from the radio's
    // old ones; the set writes the station's own as well and reads both.
    let custom = Limit {
        low_us: 1000,
        high_us: 3000,
    };
    let mut router = Router {
        station: Some(([0; 4], [DEFAULT, DEFAULT, custom, DEFAULT])),
        ..Router::at_defaults()
    };
    let mut journal = Journal::default();
    let read = set_recorded(
        &mut router,
        "radio0",
        AccessCategory::Be,
        RAISED,
        station(),
        &mut journal.recorder(),
    )
    .unwrap();
    assert_eq!(read.radio.be, RAISED);
    assert_eq!(read.station.unwrap().limits.be, RAISED);
    assert!(
        journal.kept.iter().all(|kept| kept.station),
        "station named"
    );
    // A station that does not take its write is an unconfirmed set.
    let mut gone = Router::at_defaults();
    assert!(set(&mut gone, "radio0", AccessCategory::Be, RAISED, station()).is_err());
}
